//! The resident metadata stream must decode under the production Rust schema.
use grkit_core::{effects, packaging, testing};
use grkit_serve::{
    library::{Entry, Manifest, Origin},
    runtime_backend::{decode_snapshot, SnapshotStream, StreamFrame},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

#[test]
fn resident_metadata_stream_decodes_with_exact_content_and_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("snapshot-json");
    let output = Command::new("cc")
        .args(["-std=c11", "-O2"])
        .arg("-I")
        .arg(root.join("runtime"))
        .arg(root.join("tests/runtime/library_snapshot_json_contract.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stream_executable = temporary.path().join("snapshot-stream");
    let output = Command::new("cc")
        .args(["-std=c11", "-O2"])
        .arg("-I")
        .arg(root.join("runtime"))
        .arg(root.join("tests/runtime/library_snapshot_stream_contract.c"))
        .arg(root.join("runtime/grf.c"))
        .arg("-o")
        .arg(&stream_executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for active_count in [0, 2, 64] {
        let populated = active_count != 0;
        let mut manifest = Manifest::default();
        let mut contents = BTreeMap::new();
        if populated {
            for (id, name, origin) in [
                ("alpha", "A\\\"B", Origin::Custom),
                ("beta", "Official", Origin::Official),
            ] {
                let (mut effect, icon) = effects::decode(&testing::effect(id)).unwrap();
                effect.name = name.into();
                let bytes = effect.encode(&icon).unwrap();
                let hash = packaging::sha256(&bytes);
                contents.insert(hash.clone(), bytes);
                manifest.entries.insert(
                    id.into(),
                    Entry {
                        name: name.into(),
                        content_hash: hash,
                        enabled: true,
                        origin,
                        removed: false,
                        features: 0,
                        monochrome: false,
                    },
                );
                manifest.order.push(id.into());
            }
            for i in 2..active_count {
                let id = format!("look-{i:03}");
                let bytes = testing::effect(&id);
                let hash = packaging::sha256(&bytes);
                contents.insert(hash.clone(), bytes);
                manifest.entries.insert(
                    id.clone(),
                    Entry {
                        name: "Test".into(),
                        content_hash: hash,
                        enabled: i < 40,
                        origin: Origin::Custom,
                        removed: false,
                        features: 0,
                        monochrome: false,
                    },
                );
                manifest.order.push(id);
            }
            let mut tombstone = manifest.entries["alpha"].clone();
            tombstone.enabled = false;
            tombstone.removed = true;
            manifest.entries.insert("removed".into(), tombstone.clone());
            if active_count == 64 {
                for i in manifest.entries.len()..256 {
                    manifest
                        .entries
                        .insert(format!("removed-{i:03}"), tombstone.clone());
                }
            }
        }
        let input = temporary.path().join("manifest.bin");
        fs::write(&input, manifest.encode_binary().unwrap()).unwrap();
        for (index, entry) in manifest.entries.values().enumerate() {
            if !entry.removed {
                fs::write(
                    temporary.path().join(format!("{index}.grf")),
                    &contents[&entry.content_hash],
                )
                .unwrap();
            }
        }
        let output = Command::new(&executable).arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut stream = (output.stdout.len() as u32).to_le_bytes().to_vec();
        stream.extend(&output.stdout);
        for bytes in contents.values() {
            stream.extend((bytes.len() as u32).to_le_bytes());
            stream.extend(bytes);
        }
        let pin = StreamFrame {
            token: [1; 16],
            epoch: [1, 2, 3, 4],
            card_hash: Sha256::digest(b"card-test").into(),
            generation: 7,
            total: stream.len() as u32,
            offset: 0,
            digest: Sha256::digest(&stream).into(),
            bytes: stream.clone(),
        };
        let snapshot = decode_snapshot(&stream, &pin).unwrap();
        assert_eq!(snapshot.manifest, manifest);
        assert_eq!(snapshot.contents, contents);
        let output = Command::new(&stream_executable)
            .arg(&input)
            .arg(temporary.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut remaining = output.stdout.as_slice();
        let mut assembled: Option<SnapshotStream> = None;
        let mut first = None;
        while !remaining.is_empty() {
            let length = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize;
            let frame = StreamFrame::decode(&remaining[4..4 + length]).unwrap();
            remaining = &remaining[4 + length..];
            if let Some(stream) = &mut assembled {
                stream.append(frame).unwrap();
            } else {
                first = Some(frame.clone());
                assembled = Some(SnapshotStream::begin(frame).unwrap());
            }
        }
        let streamed =
            decode_snapshot(&assembled.unwrap().finish().unwrap(), &first.unwrap()).unwrap();
        assert_eq!(streamed.manifest, manifest);
        assert_eq!(streamed.contents, contents);

        assert!(snapshot.context.dependencies_complete);
        assert_eq!(snapshot.context.hot_reload, populated);
        if populated {
            assert_eq!(snapshot.context.dependencies["alpha"], ["recipe:42"]);
            assert_eq!(snapshot.context.dependencies["beta"], ["dialSet:1"]);
            assert_eq!(
                snapshot.context.official_hashes["beta"],
                manifest.entries["beta"].content_hash
            );
            assert_eq!(snapshot.context.selected.as_deref(), Some("alpha"));
        } else {
            assert!(snapshot.loaded_ids.is_empty());
            assert!(snapshot.context.selected.is_none());
        }
    }
}
