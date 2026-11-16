//! The owner publishes metadata only for a coherent manifest/App snapshot.
use std::{path::Path, process::Command};

#[test]
fn telemetry_requires_complete_unambiguous_runtime_mapping() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("telemetry-metadata");
    let output = Command::new("cc")
        .args(["-std=c11", "-O2"])
        .arg("-I")
        .arg(root.join("runtime"))
        .arg(root.join("tests/runtime/library_telemetry_metadata_contract.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(executable).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
