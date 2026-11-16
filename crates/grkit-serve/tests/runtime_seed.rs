//! First migration preserves actual menu order without claiming official origin.
use grkit_core::{packaging, testing};
use grkit_serve::library::{Manifest, Origin};
use std::{fs, path::Path, process::Command};
#[test]
fn resident_seed_is_canonical_and_preserves_legacy_choices() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("seed");
    let output = Command::new("cc")
        .args(["-std=c11", "-O2"])
        .arg("-I")
        .arg(root.join("runtime"))
        .arg(root.join("tests/runtime/library_seed_contract.c"))
        .arg(root.join("runtime/grf.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for count in [0, 1, 40, 64] {
        let mut ids = Vec::new();
        let mut hashes = Vec::new();
        for index in 0..count {
            let id = format!("look-{:03}", count - index);
            let bytes = testing::effect(&id);
            hashes.push(packaging::sha256(&bytes));
            ids.push(id);
            fs::write(temp.path().join(format!("{index}.grf")), bytes).unwrap();
        }
        let output = Command::new(&executable)
            .arg(temp.path())
            .arg(count.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "count {count}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let manifest = Manifest::decode_binary(&output.stdout).unwrap();
        assert_eq!(manifest.encode_binary().unwrap(), output.stdout);
        assert_eq!(manifest.order, ids);
        for (index, id) in ids.iter().enumerate() {
            let entry = &manifest.entries[id];
            assert_eq!(entry.content_hash, hashes[index]);
            assert_eq!(entry.enabled, index < 40);
            assert_eq!(entry.origin, Origin::Unknown);
            assert!(!entry.removed);
        }
    }
    let output = Command::new(&executable)
        .arg(temp.path())
        .arg("41")
        .arg("capacity")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    for (index, id) in ["look-61", "look-1400"].into_iter().enumerate() {
        fs::write(
            temp.path().join(format!("{index}.grf")),
            testing::effect(id),
        )
        .unwrap();
    }
    let output = Command::new(&executable)
        .arg(temp.path())
        .arg("2")
        .arg("collision")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}
