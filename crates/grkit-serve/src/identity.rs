//! Identity discovery never writes the camera card. A missing hardware or
//! filesystem identity disables mutations instead of guessing from a path.
use anyhow::{bail, Context, Result};
use grkit_core::packaging::sha256;
use std::{fs, io::Read, path::Path};

pub fn boot_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!("boot-{}", sha256(&bytes)))
}

/// CID identifies the physical medium; the volume serial distinguishes reformats.
/// The boot sector must come from the mounted partition, not the whole disk.
pub fn card_id(cid: &str, sector: &[u8]) -> Result<String> {
    let cid = cid.trim();
    if cid.len() != 32 || !cid.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("The SD card CID is unavailable");
    }
    if sector.len() != 512 || sector[510..512] != [0x55, 0xaa] {
        bail!("The card has no supported FAT boot sector");
    }
    // exFAT specification section 3.1: VolumeSerialNumber is at offset 100.
    // https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification
    let exfat = &sector[3..11] == b"EXFAT   ";
    let serial = if exfat {
        if sector[105] != 1 {
            bail!("Unsupported exFAT major revision");
        }
        &sector[100..104]
    } else if &sector[82..90] == b"FAT32   " && sector[66] == 0x29 {
        &sector[67..71]
    } else if (&sector[54..62] == b"FAT16   " || &sector[54..62] == b"FAT12   ")
        && sector[38] == 0x29
    {
        &sector[39..43]
    } else {
        bail!("The card filesystem identity is unsupported");
    };
    if serial == [0; 4] {
        bail!("The FAT volume serial is unavailable");
    }
    // SDHCI drops the CID CRC7/end byte. RTOS reconstructs it as 01;
    // Linux commonly exposes 00. Neither byte contributes to medium identity.
    let mut identity = cid.to_ascii_lowercase().into_bytes();
    identity[30..32].copy_from_slice(b"00");
    identity.extend_from_slice(if exfat { b"exfat" } else { b"fat" });
    identity.extend_from_slice(serial);
    Ok(format!("card-{}", sha256(&identity)))
}

pub fn mounted_card_id(root: &Path) -> Result<String> {
    let mounts = fs::read_to_string("/proc/mounts")?;
    let root = root.to_str().context("Invalid card path")?;
    let device = mounts
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            (fields.next()? == root).then_some(device)
        })
        .context("The card is not mounted")?;
    let name = Path::new(device)
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid card device")?;
    // Follow only the kernel's resolved block-device topology. Partition sysfs
    // nodes have no device/cid; the parent disk owns it.
    let block = fs::canonicalize(Path::new("/sys/class/block").join(name))?;
    let disk = if block.join("partition").exists() {
        block.parent().context("Missing card device")?
    } else {
        &block
    };
    let cid = fs::read_to_string(disk.join("device/cid"))?;
    let mut sector = [0; 512];
    fs::File::open(device)?.read_exact(&mut sector)?;
    card_id(&cid, &sector)
}

/// Independent RTOS card probe: GRC1 + raw 16-byte CID + mounted FAT boot sector.
/// The owner must select and verify its mounted volume before publishing this
/// evidence. No manifest or previous HTTP card ID is involved.
pub fn owner_evidence(bytes: &[u8]) -> Result<String> {
    if bytes.len() != 532 || &bytes[..4] != b"GRC1" {
        bail!("Invalid owner card-identity evidence");
    }
    let cid: String = bytes[4..20]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    card_id(&cid, &bytes[20..])
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sector() -> [u8; 512] {
        let mut sector = [0; 512];
        sector[510..].copy_from_slice(&[0x55, 0xaa]);
        sector[82..90].copy_from_slice(b"FAT32   ");
        sector[66] = 0x29;
        sector[67..71].copy_from_slice(&[1, 2, 3, 4]);
        sector
    }
    #[test]
    fn identity_binds_medium_and_filesystem() {
        let cid = "0123456789abcdef0123456789abcdef";
        let original = card_id(cid, &sector()).unwrap();
        assert_eq!(original, card_id(&cid.to_uppercase(), &sector()).unwrap());
        let mut changed = sector();
        changed[67] = 5;
        assert_ne!(original, card_id(cid, &changed).unwrap());
        assert_ne!(
            original,
            card_id("1123456789abcdef0123456789abcdef", &sector()).unwrap()
        );
    }
    #[test]
    fn owner_evidence_normalizes_controller_crc_bytes_without_changing_identity() {
        let cid = "aa585951454d552101deadbeef006200";
        let expected = card_id(cid, &sector()).unwrap();
        assert_eq!(
            expected,
            card_id("aa585951454d552101deadbeef006201", &sector()).unwrap()
        );
        assert_eq!(
            expected,
            card_id("aa585951454d552101deadbeef0062ff", &sector()).unwrap()
        );
        let mut bytes = b"GRC1".to_vec();
        for offset in (0..32).step_by(2) {
            bytes.push(u8::from_str_radix(&cid[offset..offset + 2], 16).unwrap());
        }
        bytes.extend(sector());
        assert_eq!(owner_evidence(&bytes).unwrap(), expected);
        assert!(owner_evidence(&bytes[..531]).is_err());
        bytes[0] = 0;
        assert!(owner_evidence(&bytes).is_err());
    }
    #[test]
    fn exfat_identity_uses_its_volume_serial() {
        let cid = "0123456789abcdef0123456789abcdef";
        let mut boot = sector();
        boot[3..11].copy_from_slice(b"EXFAT   ");
        boot[105] = 1;
        boot[100..104].copy_from_slice(&[1, 2, 3, 4]);
        assert_ne!(
            card_id(cid, &boot).unwrap(),
            card_id(cid, &sector()).unwrap()
        );
        let first = card_id(cid, &boot).unwrap();
        boot[100] = 5;
        assert_ne!(first, card_id(cid, &boot).unwrap());
        boot[105] = 2;
        assert!(card_id(cid, &boot).is_err());
    }
    #[test]
    fn unsupported_or_missing_identity_fails_closed() {
        assert!(card_id("unknown", &sector()).is_err());
        assert!(card_id("0123456789abcdef0123456789abcdef", &[0; 512]).is_err());
    }
}
