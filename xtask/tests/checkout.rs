use std::{fs, process::Command};

#[test]
fn cached_generator_reads_the_invoking_checkouts_snapshot() {
    let checkout = tempfile::tempdir().unwrap();
    let manifest_dir = checkout.path().join("xtask");
    fs::create_dir(&manifest_dir).unwrap();
    fs::write(checkout.path().join("OpenAPI.yaml"), []).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_speleodb-api-xtask"))
        .env("CARGO_MANIFEST_DIR", manifest_dir)
        .arg("source-hash")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Package OpenAPI schema is empty")
    );
}
