use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use pinky_core::{MountVerifier, ObjectStore, PdfError, PdfTextExtractor, Vault};
use tokio_util::sync::CancellationToken;

struct Mounted;

impl MountVerifier for Mounted {
    fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
        Ok(true)
    }
}

/// This deliberately uses the host's real Poppler binary. The fixture is
/// repeated to make extraction long enough that cancellation interrupts an
/// active child process rather than merely cancelling before it starts.
#[tokio::test]
#[ignore = "private-host acceptance: requires PINKY_PDF_ACCEPTANCE_FIXTURE and Poppler"]
async fn interrupting_real_poppler_cleans_vault_staging() {
    let fixture = PathBuf::from(
        std::env::var("PINKY_PDF_ACCEPTANCE_FIXTURE")
            .expect("set PINKY_PDF_ACCEPTANCE_FIXTURE to a readable embedded-text PDF"),
    );
    assert!(fixture.is_file(), "PDF fixture must be a regular file");

    let pdfunite = Path::new("/usr/bin/pdfunite");
    let pdftotext = Path::new("/usr/bin/pdftotext");
    assert!(
        pdfunite.is_file(),
        "pdfunite is required for this acceptance test"
    );
    assert!(
        pdftotext.is_file(),
        "pdftotext is required for this acceptance test"
    );

    let temporary = tempfile::tempdir().unwrap();
    let combined = temporary.path().join("interrupted.pdf");
    let mut command = Command::new(pdfunite);
    // 400 copies of the 17-page system fixture create a bounded 6,800-page
    // input. It remains well below Pinky's 512 MiB input and 10,000-page limits.
    for _ in 0..400 {
        command.arg(&fixture);
    }
    let status = command.arg(&combined).status().unwrap();
    assert!(
        status.success(),
        "pdfunite must create the acceptance fixture"
    );

    let vault_root = tempfile::tempdir().unwrap();
    let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
    let objects = ObjectStore::new(vault.clone());
    let original = objects
        .put(&fs::read(&combined).unwrap(), "application/pdf")
        .unwrap();
    let extractor = PdfTextExtractor::new(pdftotext).unwrap();
    let cancellation = CancellationToken::new();
    let extraction = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            extractor
                .extract(&objects, &original.metadata.sha256, &cancellation)
                .await
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    cancellation.cancel();
    assert!(matches!(
        extraction.await.unwrap(),
        Err(PdfError::Cancelled)
    ));

    let staging = vault_root.path().join("staging/pdf");
    assert!(
        !staging.exists() || fs::read_dir(staging).unwrap().next().is_none(),
        "interrupted Poppler extraction must not leave staged input or output"
    );
}
