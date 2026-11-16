//! Cross-language identity vectors: RTOS derives the same ID as mounted Linux.
use std::{fs, path::Path, process::Command};

#[test]
fn rtos_card_identity_matches_linux_for_supported_filesystems() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    for target in ["TEST_GR3", "TEST_GR4"] {
        let exe = temp.path().join(target);
        let output = Command::new("cc")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
            .arg(format!("-D{target}"))
            .arg("-I")
            .arg(root.join("runtime"))
            .arg(root.join("tests/runtime/card_identity_contract.c"))
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cid = [
            0xaa, 0x58, 0x59, 0x51, 0x45, 0x4d, 0x55, 0x21, 1, 0xde, 0xad, 0xbe, 0xef, 0, 0x62, 0,
        ];
        let hex: String = cid.iter().map(|byte| format!("{byte:02x}")).collect();
        for kind in ["fat12", "fat16", "fat32", "exfat", "invalid", "zero"] {
            let mut sector = [0; 512];
            sector[510..].copy_from_slice(&[0x55, 0xaa]);
            match kind {
                "fat12" | "fat16" => {
                    sector[54..62].copy_from_slice(if kind == "fat12" {
                        b"FAT12   "
                    } else {
                        b"FAT16   "
                    });
                    sector[38] = 0x29;
                    sector[39..43].copy_from_slice(&[1, 2, 3, 4]);
                }
                "fat32" | "zero" => {
                    sector[82..90].copy_from_slice(b"FAT32   ");
                    sector[66] = 0x29;
                    if kind != "zero" {
                        sector[67..71].copy_from_slice(&[1, 2, 3, 4]);
                    }
                }
                "exfat" => {
                    sector[3..11].copy_from_slice(b"EXFAT   ");
                    sector[105] = 1;
                    sector[100..104].copy_from_slice(&[1, 2, 3, 4]);
                }
                _ => {}
            }
            let fixture = temp.path().join(kind);
            let mut bytes = cid.to_vec();
            bytes.extend(sector);
            fs::write(&fixture, bytes).unwrap();
            let output = Command::new(&exe).arg(fixture).output().unwrap();
            assert!(
                output.status.success(),
                "{kind}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let expected = grkit_serve::identity::card_id(&hex, &sector)
                .unwrap_or_else(|_| "unsupported".into());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap().trim(),
                expected,
                "{kind}"
            );
        }
    }
}
