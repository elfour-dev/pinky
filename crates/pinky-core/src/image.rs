//! Bounded image metadata extraction for retained local image sources.
//!
//! This parser intentionally reads only format headers and bounded metadata.
//! It never decodes or writes pixels, which keeps image ingestion cheap and
//! makes it safe to run before the isolated OCR worker is invoked.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{ObjectStore, ProcessError, ProcessSupervisor, VaultError};

pub const IMAGE_METADATA_VERSION: &str = "pinky-image-metadata-v1";
pub const MAX_IMAGE_METADATA_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_OCR_TEXT_BYTES: usize = 8 * 1024 * 1024;
const OCR_LANGUAGE_MAX_BYTES: usize = 64;
const OCR_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImageMetadataError {
    #[error("unsupported image MIME type: {0}")]
    UnsupportedMime(String),
    #[error("image is malformed or its metadata is incomplete")]
    Malformed,
    #[error("image metadata input exceeds the {MAX_IMAGE_METADATA_BYTES}-byte limit")]
    TooLarge,
}

#[derive(Debug, Error)]
pub enum ImageOcrError {
    #[error("OCR executable must be an absolute regular file: {0}")]
    InvalidExecutable(PathBuf),
    #[error("OCR language must be a short, non-empty identifier")]
    InvalidLanguage,
    #[error("OCR only supports retained image MIME types")]
    UnsupportedMime,
    #[error("OCR image is too large for the metadata/OCR safety bound")]
    ImageTooLarge,
    #[error("OCR was cancelled")]
    Cancelled,
    #[error("OCR vault unavailable: {0}")]
    Vault(#[from] VaultError),
    #[error("OCR object error: {0}")]
    Object(#[from] crate::ObjectStoreError),
    #[error("OCR I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("OCR worker process failed: {0}")]
    Process(#[from] ProcessError),
    #[error("OCR worker exited unsuccessfully with code {0:?}")]
    Failed(Option<i32>),
    #[error("OCR output exceeded the {MAX_OCR_TEXT_BYTES}-byte limit")]
    OutputTooLarge,
    #[error("OCR output was not valid UTF-8")]
    InvalidUtf8,
}

#[derive(Debug, Clone)]
pub struct ImageOcrWorker {
    executable: PathBuf,
    language: String,
}

impl ImageOcrWorker {
    pub fn new(executable: impl AsRef<Path>, language: &str) -> Result<Self, ImageOcrError> {
        let executable = executable.as_ref();
        if !executable.is_absolute()
            || !fs::symlink_metadata(executable).is_ok_and(|metadata| metadata.is_file())
        {
            return Err(ImageOcrError::InvalidExecutable(executable.to_owned()));
        }
        let language = language.trim();
        if language.is_empty()
            || language.len() > OCR_LANGUAGE_MAX_BYTES
            || language.chars().any(|character| {
                character.is_control() || character.is_whitespace() || !character.is_ascii()
            })
        {
            return Err(ImageOcrError::InvalidLanguage);
        }
        Ok(Self {
            executable: fs::canonicalize(executable)?,
            language: language.to_owned(),
        })
    }

    /// Run OCR against a retained image using a supervised process group.
    ///
    /// Both the materialised input and OCR output stay under the mounted vault
    /// staging directory and are removed on every completion path. A future
    /// container-backed worker can implement the same contract without
    /// changing callers.
    pub async fn recognize(
        &self,
        objects: &ObjectStore,
        original_hash: &str,
        mime_type: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, ImageOcrError> {
        if !is_image_mime(mime_type) {
            return Err(ImageOcrError::UnsupportedMime);
        }
        if cancellation.is_cancelled() {
            return Err(ImageOcrError::Cancelled);
        }
        let bytes = objects.read_verified(original_hash)?;
        if bytes.len() > MAX_IMAGE_METADATA_BYTES {
            return Err(ImageOcrError::ImageTooLarge);
        }
        let vault = objects.vault();
        vault.ensure_mounted()?;
        let staging = vault.resolve_internal("staging/ocr");
        if fs::symlink_metadata(&staging)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(ImageOcrError::Vault(VaultError::UnsafeLayout(staging)));
        }
        fs::create_dir_all(&staging)?;
        if !fs::canonicalize(&staging)?.starts_with(vault.root()) {
            return Err(ImageOcrError::Vault(VaultError::UnsafeLayout(staging)));
        }

        let job = uuid::Uuid::new_v4().to_string();
        let extension = image_extension(mime_type).ok_or(ImageOcrError::UnsupportedMime)?;
        let input = staging.join(format!("{job}.{extension}"));
        let output_base = staging.join(format!("{job}.result"));
        let output = output_base.with_extension("result.txt");
        if let Err(error) = write_private_file(&input, &bytes) {
            let _ = fs::remove_file(&input);
            return Err(error.into());
        }
        let result = self.run_process(&input, &output_base, cancellation).await;
        let result = match result {
            Ok(()) => read_ocr_output(&output),
            Err(error) => Err(error),
        };
        let _ = fs::remove_file(&input);
        let _ = fs::remove_file(&output);
        result
    }

    async fn run_process(
        &self,
        input: &Path,
        output_base: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(), ImageOcrError> {
        let mut command = Command::new(&self.executable);
        command
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .arg(input)
            .arg(output_base)
            .arg("--psm")
            .arg("3")
            .arg("-l")
            .arg(&self.language);
        let outcome = tokio::time::timeout(
            OCR_TIMEOUT,
            ProcessSupervisor::new().run(command, cancellation.clone()),
        )
        .await
        .map_err(|_| ImageOcrError::Failed(None))??;
        if cancellation.is_cancelled()
            || matches!(
                outcome.termination,
                crate::ProcessTermination::CancelledCooperatively
                    | crate::ProcessTermination::Sigterm
                    | crate::ProcessTermination::Sigkill
            )
        {
            return Err(ImageOcrError::Cancelled);
        }
        if outcome.exit_code != Some(0) {
            return Err(ImageOcrError::Failed(outcome.exit_code));
        }
        Ok(())
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn read_ocr_output(path: &Path) -> Result<String, ImageOcrError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(ImageOcrError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "OCR output is not a regular file",
        )));
    }
    let mut file = open_private_read(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_OCR_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_OCR_TEXT_BYTES {
        return Err(ImageOcrError::OutputTooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| ImageOcrError::InvalidUtf8)?;
    Ok(text
        .strip_prefix('\u{feff}')
        .unwrap_or(&text)
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

fn open_private_read(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

fn image_extension(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/tiff" => Some("tiff"),
        _ => None,
    }
}

fn is_image_mime(mime_type: &str) -> bool {
    image_extension(mime_type).is_some()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    pub metadata_version: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub has_alpha: Option<bool>,
    pub animated: bool,
    pub frame_count: Option<u32>,
    pub orientation: Option<u16>,
}

impl ImageMetadata {
    pub fn to_retained_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

pub fn extract_image_metadata(
    mime_type: &str,
    bytes: &[u8],
) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() > MAX_IMAGE_METADATA_BYTES {
        return Err(ImageMetadataError::TooLarge);
    }
    match mime_type {
        "image/png" => parse_png(bytes),
        "image/jpeg" => parse_jpeg(bytes),
        "image/gif" => parse_gif(bytes),
        "image/webp" => parse_webp(bytes),
        "image/tiff" => parse_tiff(bytes),
        other => Err(ImageMetadataError::UnsupportedMime(other.to_owned())),
    }
}

fn base(format: &str, width: u32, height: u32) -> ImageMetadata {
    ImageMetadata {
        metadata_version: IMAGE_METADATA_VERSION.to_owned(),
        format: format.to_owned(),
        width,
        height,
        bit_depth: None,
        channels: None,
        has_alpha: None,
        animated: false,
        frame_count: None,
        orientation: None,
    }
}

fn parse_png(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() < 26 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return Err(ImageMetadataError::Malformed);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err(ImageMetadataError::Malformed);
    }
    let bit_depth = bytes[24];
    let color_type = bytes[25];
    let (channels, has_alpha) = match color_type {
        0 => (1, false),
        2 => (3, false),
        3 => (1, false),
        4 => (2, true),
        6 => (4, true),
        _ => return Err(ImageMetadataError::Malformed),
    };
    let mut metadata = base("PNG", width, height);
    metadata.bit_depth = Some(bit_depth);
    metadata.channels = Some(channels);
    metadata.has_alpha = Some(has_alpha);
    Ok(metadata)
}

fn parse_jpeg(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() < 4 || !bytes.starts_with(b"\xff\xd8\xff") {
        return Err(ImageMetadataError::Malformed);
    }
    let mut index = 2;
    while index + 3 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        while index < bytes.len() && bytes[index] == 0xff {
            index += 1;
        }
        let marker = *bytes.get(index).ok_or(ImageMetadataError::Malformed)?;
        index += 1;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes(
            bytes
                .get(index..index + 2)
                .ok_or(ImageMetadataError::Malformed)?
                .try_into()
                .unwrap(),
        ) as usize;
        if length < 2 || index + length > bytes.len() {
            return Err(ImageMetadataError::Malformed);
        }
        if is_jpeg_frame_marker(marker) {
            if length < 8 {
                return Err(ImageMetadataError::Malformed);
            }
            let height = u16::from_be_bytes(bytes[index + 3..index + 5].try_into().unwrap()) as u32;
            let width = u16::from_be_bytes(bytes[index + 5..index + 7].try_into().unwrap()) as u32;
            let channels = bytes[index + 7];
            if width == 0 || height == 0 || channels == 0 {
                return Err(ImageMetadataError::Malformed);
            }
            let mut metadata = base("JPEG", width, height);
            metadata.bit_depth = Some(bytes[index + 2]);
            metadata.channels = Some(channels);
            metadata.has_alpha = Some(false);
            return Ok(metadata);
        }
        index += length;
    }
    Err(ImageMetadataError::Malformed)
}

fn is_jpeg_frame_marker(marker: u8) -> bool {
    matches!(
        marker,
        0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf
    )
}

fn parse_gif(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() < 13 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Err(ImageMetadataError::Malformed);
    }
    let width = u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as u32;
    let height = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as u32;
    if width == 0 || height == 0 {
        return Err(ImageMetadataError::Malformed);
    }
    let packed = bytes[10];
    let mut index: usize = 13;
    if packed & 0x80 != 0 {
        index = index.saturating_add(3usize.saturating_mul(1usize << ((packed & 0x07) + 1)));
    }
    if index > bytes.len() {
        return Err(ImageMetadataError::Malformed);
    }
    let mut frames = 0_u32;
    while index < bytes.len() {
        match bytes[index] {
            0x3b => break,
            0x21 => {
                index = index.checked_add(2).ok_or(ImageMetadataError::Malformed)?;
                skip_gif_subblocks(bytes, &mut index)?;
            }
            0x2c => {
                frames = frames.saturating_add(1);
                index = index.checked_add(10).ok_or(ImageMetadataError::Malformed)?;
                if index > bytes.len() {
                    return Err(ImageMetadataError::Malformed);
                }
                let local_packed = bytes[index - 1];
                if local_packed & 0x80 != 0 {
                    index = index.saturating_add(
                        3usize.saturating_mul(1usize << ((local_packed & 0x07) + 1)),
                    );
                }
                if index >= bytes.len() {
                    return Err(ImageMetadataError::Malformed);
                }
                index += 1;
                skip_gif_subblocks(bytes, &mut index)?;
            }
            _ => return Err(ImageMetadataError::Malformed),
        }
    }
    let mut metadata = base("GIF", width, height);
    metadata.channels = Some(3);
    metadata.has_alpha = Some(false);
    metadata.animated = frames > 1;
    metadata.frame_count = Some(frames.max(1));
    Ok(metadata)
}

fn skip_gif_subblocks(bytes: &[u8], index: &mut usize) -> Result<(), ImageMetadataError> {
    loop {
        let length = *bytes.get(*index).ok_or(ImageMetadataError::Malformed)? as usize;
        *index = index.checked_add(1).ok_or(ImageMetadataError::Malformed)?;
        if length == 0 {
            return Ok(());
        }
        *index = index
            .checked_add(length)
            .filter(|next| *next <= bytes.len())
            .ok_or(ImageMetadataError::Malformed)?;
    }
}

fn parse_webp(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() < 16 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WEBP" {
        return Err(ImageMetadataError::Malformed);
    }
    let mut index = 12;
    while index + 8 <= bytes.len() {
        let kind = &bytes[index..index + 4];
        let size = u32::from_le_bytes(bytes[index + 4..index + 8].try_into().unwrap()) as usize;
        let payload = index + 8;
        let end = payload
            .checked_add(size)
            .ok_or(ImageMetadataError::Malformed)?;
        if end > bytes.len() {
            return Err(ImageMetadataError::Malformed);
        }
        if kind == b"VP8X" {
            if size < 10 {
                return Err(ImageMetadataError::Malformed);
            }
            let width = 1 + read_u24_le(&bytes[payload + 4..payload + 7]);
            let height = 1 + read_u24_le(&bytes[payload + 7..payload + 10]);
            let mut metadata = base("WebP", width, height);
            metadata.has_alpha = Some(bytes[payload] & 0x10 != 0);
            metadata.animated = bytes[payload] & 0x02 != 0;
            metadata.frame_count = metadata
                .animated
                .then(|| count_webp_animation_frames(bytes).max(1));
            return Ok(metadata);
        }
        if kind == b"VP8 " && size >= 10 && bytes[payload + 3..payload + 6] == [0x9d, 0x01, 0x2a] {
            let width =
                u16::from_le_bytes(bytes[payload + 6..payload + 8].try_into().unwrap()) & 0x3fff;
            let height_offset = payload + 8;
            if height_offset + 2 <= bytes.len() {
                let height =
                    u16::from_le_bytes(bytes[height_offset..height_offset + 2].try_into().unwrap())
                        & 0x3fff;
                let mut metadata = base("WebP", width as u32, height as u32);
                metadata.has_alpha = Some(false);
                return Ok(metadata);
            }
        }
        index = end + (size & 1);
    }
    Err(ImageMetadataError::Malformed)
}

fn read_u24_le(bytes: &[u8]) -> u32 {
    bytes[0] as u32 | ((bytes[1] as u32) << 8) | ((bytes[2] as u32) << 16)
}

fn count_webp_animation_frames(bytes: &[u8]) -> u32 {
    let mut index = 12;
    let mut frames = 0_u32;
    while index + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[index + 4..index + 8].try_into().unwrap()) as usize;
        let payload = index + 8;
        let Some(end) = payload.checked_add(size) else {
            break;
        };
        if end > bytes.len() {
            break;
        }
        if &bytes[index..index + 4] == b"ANMF" {
            frames = frames.saturating_add(1);
        }
        index = end + (size & 1);
    }
    frames
}

fn parse_tiff(bytes: &[u8]) -> Result<ImageMetadata, ImageMetadataError> {
    if bytes.len() < 8 {
        return Err(ImageMetadataError::Malformed);
    }
    let little = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(ImageMetadataError::Malformed),
    };
    if read_u16(bytes, 2, little)? != 42 {
        return Err(ImageMetadataError::Malformed);
    }
    let ifd = read_u32(bytes, 4, little)? as usize;
    let count = read_u16(bytes, ifd, little)? as usize;
    let table_end = ifd
        .checked_add(2)
        .and_then(|offset| offset.checked_add(count.saturating_mul(12)))
        .ok_or(ImageMetadataError::Malformed)?;
    if count > 4096 || table_end > bytes.len() {
        return Err(ImageMetadataError::Malformed);
    }
    let mut width = None;
    let mut height = None;
    let mut bit_depth = None;
    let mut channels = None;
    let mut orientation = None;
    for entry in 0..count {
        let offset = ifd + 2 + entry * 12;
        let tag = read_u16(bytes, offset, little)?;
        let field_type = read_u16(bytes, offset + 2, little)?;
        let value_count = read_u32(bytes, offset + 4, little)?;
        let value = tiff_value(bytes, offset + 8, field_type, value_count, little)?;
        match tag {
            256 => width = Some(value),
            257 => height = Some(value),
            258 => bit_depth = Some(value.min(u8::MAX as u32) as u8),
            277 => channels = Some(value.min(u8::MAX as u32) as u8),
            274 => orientation = Some(value.min(u16::MAX as u32) as u16),
            _ => {}
        }
    }
    let (width, height) = (width, height);
    if width.is_none_or(|value| value == 0) || height.is_none_or(|value| value == 0) {
        return Err(ImageMetadataError::Malformed);
    }
    let mut metadata = base("TIFF", width.unwrap(), height.unwrap());
    metadata.bit_depth = bit_depth;
    metadata.channels = channels;
    metadata.has_alpha = channels.map(|value| value == 4);
    metadata.orientation = orientation;
    Ok(metadata)
}

fn read_u16(bytes: &[u8], offset: usize, little: bool) -> Result<u16, ImageMetadataError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(ImageMetadataError::Malformed)?;
    Ok(if little {
        u16::from_le_bytes(value.try_into().unwrap())
    } else {
        u16::from_be_bytes(value.try_into().unwrap())
    })
}

fn read_u32(bytes: &[u8], offset: usize, little: bool) -> Result<u32, ImageMetadataError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(ImageMetadataError::Malformed)?;
    Ok(if little {
        u32::from_le_bytes(value.try_into().unwrap())
    } else {
        u32::from_be_bytes(value.try_into().unwrap())
    })
}

fn tiff_value(
    bytes: &[u8],
    inline_offset: usize,
    field_type: u16,
    count: u32,
    little: bool,
) -> Result<u32, ImageMetadataError> {
    if count == 0 {
        return Err(ImageMetadataError::Malformed);
    }
    let unit = match field_type {
        3 => 2_u64,
        4 => 4_u64,
        _ => return Err(ImageMetadataError::Malformed),
    };
    let byte_count = unit
        .checked_mul(count as u64)
        .ok_or(ImageMetadataError::Malformed)?;
    let offset = if byte_count <= 4 {
        inline_offset
    } else {
        read_u32(bytes, inline_offset, little)? as usize
    };
    match field_type {
        3 => read_u16(bytes, offset, little).map(u32::from),
        4 => read_u32(bytes, offset, little),
        _ => Err(ImageMetadataError::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MountVerifier, Vault};
    use std::path::Path;

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[test]
    fn extracts_png_metadata_without_decoding_pixels() {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&[0, 0, 4, 0, 0, 0, 3, 0, 8, 6, 0, 0, 0, 0]);
        let metadata = extract_image_metadata("image/png", &bytes).unwrap();
        assert_eq!(metadata.width, 1024);
        assert_eq!(metadata.height, 768);
        assert_eq!(metadata.channels, Some(4));
        assert_eq!(metadata.has_alpha, Some(true));
    }

    #[test]
    fn extracts_gif_dimensions_and_frame_count() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&[2, 0, 3, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[0x2c, 0, 0, 0, 0, 2, 0, 3, 0, 0, 0, 2, 1, 0, 0]);
        bytes.extend_from_slice(&[0x3b]);
        let metadata = extract_image_metadata("image/gif", &bytes).unwrap();
        assert_eq!((metadata.width, metadata.height), (2, 3));
        assert_eq!(metadata.frame_count, Some(1));
    }

    #[test]
    fn extracts_jpeg_dimensions_from_a_sof_marker() {
        let bytes = [
            0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0f, 0x08, 0x00, 0x03, 0x00, 0x04, 0x03, 0x01, 0x11,
            0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00,
        ];
        let metadata = extract_image_metadata("image/jpeg", &bytes).unwrap();
        assert_eq!((metadata.width, metadata.height), (4, 3));
        assert_eq!(metadata.channels, Some(3));
    }

    #[test]
    fn extracts_webp_dimensions_from_vp8x() {
        let bytes = [
            b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P', b'V', b'P', b'8', b'X', 10,
            0, 0, 0, 0x10, 0, 0, 0, 0xff, 0x03, 0, 0xff, 0x01, 0,
        ];
        let metadata = extract_image_metadata("image/webp", &bytes).unwrap();
        assert_eq!((metadata.width, metadata.height), (1024, 512));
        assert_eq!(metadata.has_alpha, Some(true));
    }

    #[test]
    fn extracts_tiff_dimensions_and_orientation() {
        let mut bytes = b"II*\0\x08\0\0\0\x05\0".to_vec();
        for (tag, field_type, value) in [(256_u16, 4_u16, 4_u32), (257, 4, 3)] {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&field_type.to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for (tag, value) in [(258_u16, 8_u16), (277, 3), (274, 1)] {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&3_u16.to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&[0, 0]);
        }
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let metadata = extract_image_metadata("image/tiff", &bytes).unwrap();
        assert_eq!((metadata.width, metadata.height), (4, 3));
        assert_eq!(metadata.channels, Some(3));
        assert_eq!(metadata.orientation, Some(1));
    }

    #[test]
    fn rejects_truncated_or_unknown_images() {
        assert_eq!(
            extract_image_metadata("image/png", b"short"),
            Err(ImageMetadataError::Malformed)
        );
        assert_eq!(
            extract_image_metadata("image/bmp", b"BM"),
            Err(ImageMetadataError::UnsupportedMime("image/bmp".into()))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn supervised_ocr_keeps_materialized_input_and_output_inside_the_vault() {
        use std::os::unix::fs::PermissionsExt;

        let vault_root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let stored = objects.put(b"retained image bytes", "image/png").unwrap();
        let executable = vault_root.path().join("fake-ocr.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'OCR result\\n' > \"$2.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let worker = ImageOcrWorker::new(&executable, "eng").unwrap();
        let output = worker
            .recognize(
                &objects,
                &stored.metadata.sha256,
                "image/png",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(output, "OCR result\n");
        let staging = vault_root.path().join("staging/ocr");
        assert!(fs::read_dir(&staging)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_during_ocr_terminates_the_worker_and_cleans_staging() {
        use std::os::unix::fs::PermissionsExt;

        let vault_root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let stored = objects.put(b"retained image bytes", "image/png").unwrap();
        let executable = vault_root.path().join("slow-ocr.sh");
        fs::write(&executable, "#!/bin/sh\n/bin/sleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let worker = ImageOcrWorker::new(&executable, "eng").unwrap();
        let cancellation = CancellationToken::new();
        let running = {
            let objects = objects.clone();
            let worker = worker.clone();
            let hash = stored.metadata.sha256.clone();
            let cancellation = cancellation.clone();
            tokio::spawn(async move {
                worker
                    .recognize(&objects, &hash, "image/png", &cancellation)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancellation.cancel();
        assert!(matches!(
            running.await.unwrap(),
            Err(ImageOcrError::Cancelled)
        ));
        let staging = vault_root.path().join("staging/ocr");
        assert!(fs::read_dir(&staging)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_ocr_output_is_rejected_and_removed() {
        use std::os::unix::fs::PermissionsExt;

        let vault_root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let stored = objects.put(b"retained image bytes", "image/png").unwrap();
        let executable = vault_root.path().join("large-ocr.sh");
        fs::write(
            &executable,
            "#!/bin/sh\n/usr/bin/head -c 8388609 /dev/zero > \"$2.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let worker = ImageOcrWorker::new(&executable, "eng").unwrap();
        assert!(matches!(
            worker
                .recognize(
                    &objects,
                    &stored.metadata.sha256,
                    "image/png",
                    &CancellationToken::new(),
                )
                .await,
            Err(ImageOcrError::OutputTooLarge)
        ));
        let staging = vault_root.path().join("staging/ocr");
        assert!(fs::read_dir(&staging)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_ocr_never_materializes_retained_pixels() {
        use std::os::unix::fs::PermissionsExt;

        let vault_root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let stored = objects.put(b"retained image bytes", "image/png").unwrap();
        let executable = vault_root.path().join("fake-ocr.sh");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let worker = ImageOcrWorker::new(&executable, "eng").unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            worker
                .recognize(
                    &objects,
                    &stored.metadata.sha256,
                    "image/png",
                    &cancellation,
                )
                .await,
            Err(ImageOcrError::Cancelled)
        ));
        let staging = vault_root.path().join("staging/ocr");
        assert!(fs::read_dir(&staging)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));
    }

    #[test]
    fn validates_ocr_executable_and_language_before_running() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("ocr");
        fs::write(&executable, b"#!/bin/sh").unwrap();
        assert!(matches!(
            ImageOcrWorker::new("relative/ocr", "eng"),
            Err(ImageOcrError::InvalidExecutable(_))
        ));
        assert!(matches!(
            ImageOcrWorker::new(&executable, "en g"),
            Err(ImageOcrError::InvalidLanguage)
        ));
    }
}
