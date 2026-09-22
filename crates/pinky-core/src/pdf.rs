//! Bounded embedded-text extraction for retained PDF sources.
//!
//! PDF parsing is delegated to supervised Poppler workers. The original PDF
//! remains in the encrypted object store; only vault-owned staging copies are
//! exposed to the workers and their outputs are removed after the extraction
//! attempt. Page breaks emitted by Poppler are retained so the ingestion layer
//! can attach page-aware coordinates to searchable chunks. Image-only pages
//! can additionally be rendered with `pdftoppm` and OCRed with the existing
//! bounded Tesseract worker when those runtimes are available.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    time::Duration,
};

use thiserror::Error;
use tokio::{process::Command, time::timeout};
use tokio_util::sync::CancellationToken;

use crate::{
    extract_image_metadata, ImageOcrError, ImageOcrWorker, ObjectStore, ProcessError,
    ProcessSupervisor, VaultError,
};

pub const PDF_EXTRACTION_VERSION: &str = "pinky-pdf-text-v2";
pub const MAX_PDF_INPUT_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_PDF_TEXT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PDF_PAGES: usize = 10_000;
pub const MAX_PDF_RENDERED_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PDF_RENDERED_PIXELS: u64 = 100_000_000;
pub const PDF_WORKER_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const PDF_WORKER_FILE_BYTES: u64 = 128 * 1024 * 1024;
const PDF_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Error)]
pub enum PdfError {
    #[error("PDF extractor must be an absolute regular file: {0}")]
    InvalidExecutable(PathBuf),
    #[error("PDF renderer must be an absolute regular file: {0}")]
    InvalidRenderer(PathBuf),
    #[error("PDF input exceeds the {MAX_PDF_INPUT_BYTES}-byte safety limit")]
    InputTooLarge,
    #[error("PDF extractor was cancelled")]
    Cancelled,
    #[error("PDF vault unavailable: {0}")]
    Vault(#[from] VaultError),
    #[error("PDF object error: {0}")]
    Object(#[from] crate::ObjectStoreError),
    #[error("PDF I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("PDF worker process failed: {0}")]
    Process(#[from] ProcessError),
    #[error("PDF worker timed out after 180 seconds")]
    Timeout,
    #[error("PDF worker exited unsuccessfully with code {0:?}")]
    Failed(Option<i32>),
    #[error("PDF page renderer exited unsuccessfully with code {0:?}")]
    RenderFailed(Option<i32>),
    #[error("PDF text output exceeded the {MAX_PDF_TEXT_BYTES}-byte safety limit")]
    OutputTooLarge,
    #[error("PDF text output was not a regular file inside the vault")]
    InvalidOutput,
    #[error("PDF text output was not valid UTF-8")]
    InvalidUtf8,
    #[error("PDF contains more than the {MAX_PDF_PAGES}-page safety limit")]
    TooManyPages,
    #[error("rendered PDF page exceeded the {MAX_PDF_RENDERED_IMAGE_BYTES}-byte safety limit")]
    RenderedImageTooLarge,
    #[error("rendered PDF page is not a valid bounded image")]
    InvalidRenderedImage,
    #[error("OCR failed for PDF page {page}: {source}")]
    PageOcr {
        page: usize,
        #[source]
        source: Box<ImageOcrError>,
    },
}

#[derive(Debug, Clone)]
pub struct PdfTextExtractor {
    executable: PathBuf,
    renderer: Option<PathBuf>,
    ocr_worker: Option<std::sync::Arc<ImageOcrWorker>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfExtraction {
    pub text: String,
    pub ocr_pages: Vec<usize>,
}

impl PdfTextExtractor {
    pub fn new(executable: impl AsRef<Path>) -> Result<Self, PdfError> {
        let executable = executable.as_ref();
        if !executable.is_absolute()
            || !fs::symlink_metadata(executable).is_ok_and(|metadata| metadata.is_file())
        {
            return Err(PdfError::InvalidExecutable(executable.to_owned()));
        }
        Ok(Self {
            executable: fs::canonicalize(executable)?,
            renderer: None,
            ocr_worker: None,
        })
    }

    pub fn with_renderer(mut self, renderer: impl AsRef<Path>) -> Result<Self, PdfError> {
        let renderer = renderer.as_ref();
        if !renderer.is_absolute()
            || !fs::symlink_metadata(renderer).is_ok_and(|metadata| metadata.is_file())
        {
            return Err(PdfError::InvalidRenderer(renderer.to_owned()));
        }
        self.renderer = Some(fs::canonicalize(renderer)?);
        Ok(self)
    }

    pub fn with_page_ocr(mut self, worker: ImageOcrWorker) -> Self {
        self.ocr_worker = Some(std::sync::Arc::new(worker));
        self
    }

    /// Extract embedded text from one retained PDF and, when configured,
    /// render/OCR pages for which Poppler found no embedded text. Every
    /// rendered page is temporary vault staging data and is removed before
    /// this method returns.
    pub async fn extract(
        &self,
        objects: &ObjectStore,
        original_hash: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, PdfError> {
        Ok(self
            .extract_detailed(objects, original_hash, cancellation)
            .await?
            .text)
    }

    /// Extract a retained PDF while retaining provenance about pages that
    /// required rendered-image OCR. The simpler [`Self::extract`] API remains
    /// available for callers that only need the normalized text.
    pub async fn extract_detailed(
        &self,
        objects: &ObjectStore,
        original_hash: &str,
        cancellation: &CancellationToken,
    ) -> Result<PdfExtraction, PdfError> {
        if cancellation.is_cancelled() {
            return Err(PdfError::Cancelled);
        }
        let bytes = match objects.read_verified_limited(original_hash, MAX_PDF_INPUT_BYTES) {
            Ok(bytes) => bytes,
            Err(crate::ObjectStoreError::ReadLimitExceeded { .. }) => {
                return Err(PdfError::InputTooLarge)
            }
            Err(error) => return Err(PdfError::Object(error)),
        };
        let vault = objects.vault();
        vault.ensure_mounted()?;
        let staging = vault.resolve_internal("staging/pdf");
        if fs::symlink_metadata(&staging)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(PdfError::Vault(VaultError::UnsafeLayout(staging)));
        }
        fs::create_dir_all(&staging)?;
        if !fs::canonicalize(&staging)?.starts_with(vault.root()) {
            return Err(PdfError::Vault(VaultError::UnsafeLayout(staging)));
        }

        let job = uuid::Uuid::new_v4();
        let input = staging.join(format!("{job}.pdf"));
        let output = staging.join(format!("{job}.txt"));
        write_private_file(&input, &bytes)?;

        let result = async {
            let mut command = Command::new(&self.executable);
            command.args(["-layout", "-enc", "UTF-8"]);
            command.arg(&input).arg(&output);
            configure_pdf_worker_limits(&mut command);
            let outcome = timeout(
                PDF_TIMEOUT,
                ProcessSupervisor::new().run(command, cancellation.clone()),
            )
            .await
            .map_err(|_| PdfError::Timeout)??;
            if cancellation.is_cancelled() {
                return Err(PdfError::Cancelled);
            }
            if outcome.exit_code != Some(0) {
                return Err(PdfError::Failed(outcome.exit_code));
            }
            let metadata = fs::symlink_metadata(&output)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || !fs::canonicalize(&output)?.starts_with(vault.root())
            {
                return Err(PdfError::InvalidOutput);
            }
            if metadata.len() > MAX_PDF_TEXT_BYTES as u64 {
                return Err(PdfError::OutputTooLarge);
            }
            let text = fs::read(&output).map_err(PdfError::Io)?;
            let text = String::from_utf8(text).map_err(|_| PdfError::InvalidUtf8)?;
            if text.matches('\u{000c}').count() + 1 > MAX_PDF_PAGES {
                return Err(PdfError::TooManyPages);
            }
            let text = normalize_pdf_text(&text);
            self.ocr_image_only_pages(vault, &staging, &input, &text, cancellation)
                .await
        }
        .await;
        let _ = fs::remove_file(&input);
        let _ = fs::remove_file(&output);
        result
    }

    /// Synchronous bridge for the existing ingestion worker, which already
    /// runs outside the async runtime in a bounded blocking task.
    pub fn extract_blocking(
        &self,
        objects: &ObjectStore,
        original_hash: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, PdfError> {
        Ok(self
            .extract_blocking_detailed(objects, original_hash, cancellation)?
            .text)
    }

    pub fn extract_blocking_detailed(
        &self,
        objects: &ObjectStore,
        original_hash: &str,
        cancellation: &CancellationToken,
    ) -> Result<PdfExtraction, PdfError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(PdfError::Io)?
            .block_on(self.extract_detailed(objects, original_hash, cancellation))
    }

    async fn ocr_image_only_pages(
        &self,
        vault: &crate::Vault,
        staging: &Path,
        input: &Path,
        text: &str,
        cancellation: &CancellationToken,
    ) -> Result<PdfExtraction, PdfError> {
        let mut pages = text.split('\u{000c}').collect::<Vec<_>>();
        // Poppler versions commonly emit a terminal page-break marker. It is
        // a delimiter, not an additional page; retain one empty page for a
        // genuinely blank one-page document, but avoid rendering a phantom
        // page past the end of a multi-page document.
        if pages.len() > 1 && pages.last() == Some(&"") {
            pages.pop();
        }
        if pages.len() > MAX_PDF_PAGES {
            return Err(PdfError::TooManyPages);
        }
        let (Some(renderer), Some(ocr_worker)) = (&self.renderer, &self.ocr_worker) else {
            return Ok(PdfExtraction {
                text: text.to_owned(),
                ocr_pages: Vec::new(),
            });
        };
        let has_embedded_text = pages.iter().any(|page| !page.trim().is_empty());
        let objects = ObjectStore::new(vault.clone());
        let mut rendered_pages = Vec::with_capacity(pages.len());
        let mut ocr_pages = Vec::new();
        for (index, page_text) in pages.iter().enumerate() {
            if cancellation.is_cancelled() {
                return Err(PdfError::Cancelled);
            }
            if !page_text.trim().is_empty() {
                rendered_pages.push((*page_text).to_owned());
                continue;
            }
            let page_number = index + 1;
            let base = staging.join(uuid::Uuid::new_v4().to_string());
            let rendered = base.with_extension("png");
            let mut command = Command::new(renderer);
            command.args(["-png", "-singlefile", "-f"]);
            command.arg(page_number.to_string());
            command.args(["-l"]);
            command.arg(page_number.to_string());
            command.args(["-r", "150"]);
            command.arg(input).arg(&base);
            configure_pdf_worker_limits(&mut command);
            let outcome = match timeout(
                PDF_TIMEOUT,
                ProcessSupervisor::new().run(command, cancellation.clone()),
            )
            .await
            {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(error)) => {
                    let _ = fs::remove_file(&rendered);
                    return Err(PdfError::Process(error));
                }
                Err(_) => {
                    let _ = fs::remove_file(&rendered);
                    return Err(PdfError::Timeout);
                }
            };
            if cancellation.is_cancelled() {
                let _ = fs::remove_file(&rendered);
                return Err(PdfError::Cancelled);
            }
            if outcome.exit_code != Some(0) {
                let _ = fs::remove_file(&rendered);
                return Err(PdfError::RenderFailed(outcome.exit_code));
            }
            let rendered_result = async {
                let metadata = fs::symlink_metadata(&rendered)?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || !fs::canonicalize(&rendered)?.starts_with(vault.root())
                {
                    return Err(PdfError::InvalidRenderedImage);
                }
                if metadata.len() > MAX_PDF_RENDERED_IMAGE_BYTES as u64 {
                    return Err(PdfError::RenderedImageTooLarge);
                }
                let bytes = fs::read(&rendered)?;
                let image = extract_image_metadata("image/png", &bytes)
                    .map_err(|_| PdfError::InvalidRenderedImage)?;
                if u64::from(image.width) * u64::from(image.height) > MAX_PDF_RENDERED_PIXELS {
                    return Err(PdfError::InvalidRenderedImage);
                }
                let ocr = ocr_worker
                    .recognize_staged(&objects, &rendered, "image/png", cancellation)
                    .await
                    .map_err(|source| PdfError::PageOcr {
                        page: page_number,
                        source: Box::new(source),
                    })?;
                Ok::<String, PdfError>(normalize_pdf_text(&ocr))
            }
            .await;
            let _ = fs::remove_file(&rendered);
            match rendered_result {
                Ok(page_text) => {
                    if !page_text.trim().is_empty() {
                        ocr_pages.push(page_number);
                    }
                    rendered_pages.push(page_text);
                }
                Err(_error) if has_embedded_text => rendered_pages.push(String::new()),
                Err(error) => return Err(error),
            }
        }
        let combined = rendered_pages.join("\u{000c}");
        if combined.len() > MAX_PDF_TEXT_BYTES {
            return Err(PdfError::OutputTooLarge);
        }
        Ok(PdfExtraction {
            text: combined,
            ocr_pages,
        })
    }
}

/// Apply conservative Linux resource limits in addition to the async
/// deadline. Poppler workers are untrusted parsers from Pinky's perspective;
/// the process supervisor still owns cancellation and descendant cleanup.
fn configure_pdf_worker_limits(command: &mut Command) {
    unsafe {
        command.as_std_mut().pre_exec(|| {
            let memory = libc::rlimit {
                rlim_cur: PDF_WORKER_MEMORY_BYTES as libc::rlim_t,
                rlim_max: PDF_WORKER_MEMORY_BYTES as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0 {
                return Err(io::Error::last_os_error());
            }
            let file_size = libc::rlimit {
                rlim_cur: PDF_WORKER_FILE_BYTES as libc::rlim_t,
                rlim_max: PDF_WORKER_FILE_BYTES as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &file_size) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn normalize_pdf_text(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageOcrWorker, MountVerifier, Vault};
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn extracts_page_breaks_from_a_bounded_worker() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();
        let executable = root.path().join("pdftotext-fixture.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'Page one\\fPage two\\n' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();
        let text = extractor
            .extract(
                &objects,
                &original.metadata.sha256,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(text, "Page one\u{000c}Page two\n");
    }

    #[tokio::test]
    async fn rejects_oversized_pdf_text_output() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();
        let executable = root.path().join("pdftotext-large-fixture.sh");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nhead -c {} /dev/zero > \"$last\"\n",
                MAX_PDF_TEXT_BYTES + 1
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();
        assert!(matches!(
            extractor
                .extract(
                    &objects,
                    &original.metadata.sha256,
                    &CancellationToken::new()
                )
                .await,
            Err(PdfError::OutputTooLarge)
        ));
    }

    #[tokio::test]
    async fn rejects_an_oversized_pdf_object_before_worker_materialisation() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault.clone());
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();
        let metadata_path = vault.resolve_internal(format!(
            "objects/{}/{}.meta.json",
            &original.metadata.sha256[..2],
            original.metadata.sha256
        ));
        let mut metadata = original.metadata.clone();
        metadata.uncompressed_length = (MAX_PDF_INPUT_BYTES as u64) + 1;
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

        let executable = root.path().join("pdftotext-unreachable-fixture.sh");
        fs::write(&executable, "#!/bin/sh\nexit 91\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();
        assert!(matches!(
            extractor
                .extract(
                    &objects,
                    &original.metadata.sha256,
                    &CancellationToken::new()
                )
                .await,
            Err(PdfError::InputTooLarge)
        ));
    }

    #[tokio::test]
    async fn classifies_malformed_and_encrypted_worker_failures() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let malformed = objects.put(b"%PDF malformed", "application/pdf").unwrap();
        let encrypted = objects.put(b"%PDF ENCRYPTED", "application/pdf").unwrap();
        let executable = root.path().join("pdftotext-failure-fixture.sh");
        fs::write(
            &executable,
            "#!/bin/sh\ncase \"$(cat \"$4\")\" in\n  *ENCRYPTED*) exit 13;;\n  *) exit 17;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();

        assert!(matches!(
            extractor
                .extract(
                    &objects,
                    &malformed.metadata.sha256,
                    &CancellationToken::new()
                )
                .await,
            Err(PdfError::Failed(Some(17)))
        ));
        assert!(matches!(
            extractor
                .extract(
                    &objects,
                    &encrypted.metadata.sha256,
                    &CancellationToken::new()
                )
                .await,
            Err(PdfError::Failed(Some(13)))
        ));
        assert!(!root
            .path()
            .join("staging/pdf")
            .read_dir()
            .unwrap()
            .any(|entry| {
                entry
                    .ok()
                    .map(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "pdf")
                    })
                    .unwrap_or(false)
            }));
    }

    #[tokio::test]
    async fn rejects_pdf_page_count_above_the_bound() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();
        let executable = root.path().join("pdftotext-pages-fixture.sh");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\ni=0\nwhile [ $i -le {} ]; do printf '\\f' >> \"$last\"; i=$((i + 1)); done\n",
                MAX_PDF_PAGES
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();
        assert!(matches!(
            extractor
                .extract(
                    &objects,
                    &original.metadata.sha256,
                    &CancellationToken::new()
                )
                .await,
            Err(PdfError::TooManyPages)
        ));
    }

    #[tokio::test]
    async fn rejects_an_oversized_rendered_page_before_ocr() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();
        let extractor = root.path().join("pdftotext-empty-fixture.sh");
        fs::write(
            &extractor,
            "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf '' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&extractor, fs::Permissions::from_mode(0o700)).unwrap();
        let renderer = root.path().join("pdftoppm-large-fixture.sh");
        fs::write(
            &renderer,
            format!(
                "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\ntruncate -s {} \"$last.png\"\n",
                MAX_PDF_RENDERED_IMAGE_BYTES + 1
            ),
        )
        .unwrap();
        fs::set_permissions(&renderer, fs::Permissions::from_mode(0o700)).unwrap();
        let ocr = root.path().join("tesseract-unused-fixture.sh");
        fs::write(&ocr, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&ocr, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&extractor)
            .unwrap()
            .with_renderer(&renderer)
            .unwrap()
            .with_page_ocr(ImageOcrWorker::new(&ocr, "eng").unwrap());

        assert!(matches!(
            extractor
                .extract(
                    &objects,
                    &original.metadata.sha256,
                    &CancellationToken::new()
                )
                .await,
            Err(PdfError::RenderedImageTooLarge)
        ));
    }

    #[tokio::test]
    async fn cancellation_cleans_pdf_worker_staging() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();
        let executable = root.path().join("pdftotext-sleep-fixture.sh");
        fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();
        let cancellation = CancellationToken::new();
        let task = tokio::spawn({
            let cancellation = cancellation.clone();
            async move {
                extractor
                    .extract(&objects, &original.metadata.sha256, &cancellation)
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancellation.cancel();
        assert!(matches!(task.await.unwrap(), Err(PdfError::Cancelled)));
        assert!(!root
            .path()
            .join("staging/pdf")
            .read_dir()
            .unwrap()
            .any(|entry| {
                entry
                    .ok()
                    .map(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "pdf")
                    })
                    .unwrap_or(false)
            }));
    }

    #[tokio::test]
    async fn renders_and_ocr_image_only_pages_inside_vault_staging() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let original = objects.put(b"%PDF-1.7 fixture", "application/pdf").unwrap();

        let extractor = root.path().join("pdftotext-fixture.sh");
        fs::write(
            &extractor,
            "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'Page one\\f\\fPage three\\n' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&extractor, fs::Permissions::from_mode(0o700)).unwrap();

        // The metadata parser intentionally accepts this bounded header-only
        // fixture; no pixel decoder is needed for the worker boundary test.
        let png = root.path().join("rendered.png");
        let mut png_bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png_bytes.extend_from_slice(&[0, 0, 2, 0, 0, 0, 2, 0, 8, 6, 0, 0, 0, 0]);
        fs::write(&png, png_bytes).unwrap();

        let renderer = root.path().join("pdftoppm-fixture.sh");
        fs::write(
            &renderer,
            format!(
                "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\ncp '{}' \"$last.png\"\n",
                png.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&renderer, fs::Permissions::from_mode(0o700)).unwrap();

        let ocr = root.path().join("tesseract-fixture.sh");
        fs::write(
            &ocr,
            "#!/bin/sh\nprintf 'rendered page text\\n' > \"$2.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&ocr, fs::Permissions::from_mode(0o700)).unwrap();

        let worker = ImageOcrWorker::new(&ocr, "eng").unwrap();
        let extractor = PdfTextExtractor::new(&extractor)
            .unwrap()
            .with_renderer(&renderer)
            .unwrap()
            .with_page_ocr(worker);
        let details = extractor
            .extract_detailed(
                &objects,
                &original.metadata.sha256,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(details.ocr_pages, vec![2]);
        assert_eq!(
            details.text,
            "Page one\u{000c}rendered page text\n\u{000c}Page three\n"
        );
        assert!(!root
            .path()
            .join("staging/pdf")
            .read_dir()
            .unwrap()
            .any(|entry| {
                entry
                    .ok()
                    .and_then(|entry| entry.path().extension().map(|extension| extension == "png"))
                    .unwrap_or(false)
            }));
    }
}
