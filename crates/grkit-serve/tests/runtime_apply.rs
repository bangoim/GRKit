//! The native staged candidate parser consumes the production binary bundle.
use grkit_core::{packaging, testing};
use grkit_serve::library::{Entry, Manifest, Origin};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

#[test]
fn resident_apply_parser_accepts_arbitrary_chunk_boundaries_without_extra_staging() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("apply-stream");
    let output = Command::new("cc")
        .args(["-std=c11", "-O2"])
        .arg("-I")
        .arg(root.join("runtime"))
        .arg(root.join("tests/runtime/library_apply_stream_contract.c"))
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
    for identities in [0, 1, 256] {
        let mut manifest = Manifest::default();
        let mut content = Vec::new();
        if identities > 0 {
            content = testing::effect("sample");
            let entry = Entry {
                name: "Test".into(),
                content_hash: packaging::sha256(&content),
                enabled: true,
                origin: Origin::Custom,
                removed: false,
                features: 0,
                monochrome: false,
            };
            manifest.entries.insert("sample".into(), entry.clone());
            manifest.order.push("sample".into());
            for i in 1..identities {
                let mut removed = entry.clone();
                removed.enabled = false;
                removed.removed = true;
                manifest.entries.insert(format!("removed-{i:03}"), removed);
            }
        }
        let bytes = manifest.encode_binary().unwrap();
        let mut bundle = Sha256::digest(Manifest::default().encode_binary().unwrap()).to_vec();
        bundle.extend((bytes.len() as u32).to_le_bytes());
        bundle.extend(&bytes);
        bundle.extend((content.len() as u32).to_le_bytes());
        bundle.extend(&content);
        bundle.extend(1u32.to_le_bytes());
        let input = temp.path().join("candidate.bin");
        fs::write(&input, &bundle).unwrap();
        for chunk in [1, 31, 32, 35, 36, 37, 127, 128, 8192, 8576] {
            let output = Command::new(&executable)
                .arg(&input)
                .arg(chunk.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "identities {identities}, chunk {chunk}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut expected = (bytes.len() as u32).to_le_bytes().to_vec();
            expected.extend((content.len() as u32).to_le_bytes());
            expected.extend(1u32.to_le_bytes());
            expected.extend(&bytes);
            expected.extend(&content);
            assert_eq!(output.stdout, expected);
        }
    }
}
