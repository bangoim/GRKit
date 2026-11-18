//! Identity discovery writes the camera card only to create its marker once.
//! A missing hardware or card identity disables mutations instead of
//! guessing from a path.
use anyhow::{bail, Context, Result};
use grkit_core::packaging::sha256;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

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

/// The CID as 32 lowercase hex digits, with the CRC7/end byte as 00 (see
/// `card_id`).
pub fn normalized_cid(cid: &str) -> Result<String> {
    let cid = cid.trim();
    if cid.len() != 32
        || !cid.bytes().all(|c| c.is_ascii_hexdigit())
        || cid[..30].bytes().all(|c| c == b'0')
    {
        bail!("The SD card CID is unavailable");
    }
    let mut cid = cid.to_ascii_lowercase();
    cid.replace_range(30..32, "00");
    Ok(cid)
}

/// The card ID that the service publishes on every kind of start: the CID of
/// the medium and the random marker of `card_marker`. A format of the card
/// removes the marker, and the CID keeps a copied marker from naming another
/// card. Both inputs are in RAM or on the card on a start with or without the
/// fast path.
pub fn public_card_id(cid: &str, marker: &str) -> Result<String> {
    let cid = normalized_cid(cid)?;
    if marker.len() != 32
        || !marker
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    {
        bail!("The card marker is invalid");
    }
    Ok(format!(
        "card-{}",
        sha256(format!("grkit-card-1:{cid}:{marker}").as_bytes())
    ))
}

/// The marker file that completes the card identity.
pub const MARKER: &str = "GRKIT/CARD.ID";

/// Read the card marker, or create it once with 128 random bits. This is the
/// only write of identity discovery: one small file in the life of a card.
pub fn card_marker(root: &Path) -> Result<String> {
    let path = root.join(MARKER);
    let read = |path: &Path| -> Result<String> {
        let mut text = String::new();
        fs::File::open(path)?.take(64).read_to_string(&mut text)?;
        let marker = text.trim().to_owned();
        if marker.len() != 32
            || !marker
                .bytes()
                .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        {
            bail!("The card marker is invalid");
        }
        Ok(marker)
    };
    match read(&path) {
        Ok(marker) => return Ok(marker),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(error) => return Err(error),
    }
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let marker: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .context("Create the card marker")?;
    file.write_all(marker.as_bytes())?;
    file.sync_all()?;
    drop(file);
    // Read it back: a card that drops the write must not get a new
    // identity at each start.
    let stored = read(&path)?;
    if stored != marker {
        bail!("The card marker did not persist");
    }
    Ok(stored)
}

/// Independent RTOS card probe: GRC1 + raw 16-byte CID + mounted FAT boot sector.
/// The owner must select and verify its mounted volume before publishing this
/// evidence. No manifest or previous HTTP card ID is involved.
/// Returns the owner's card ID and the normalized CID.
pub fn owner_evidence(bytes: &[u8]) -> Result<(String, String)> {
    if bytes.len() != 532 || &bytes[..4] != b"GRC1" {
        bail!("Invalid owner card-identity evidence");
    }
    let cid: String = bytes[4..20]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((card_id(&cid, &bytes[20..])?, normalized_cid(&cid)?))
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
        assert_eq!(owner_evidence(&bytes).unwrap().0, expected);
        assert_eq!(
            owner_evidence(&bytes).unwrap().1,
            "aa585951454d552101deadbeef006200"
        );
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
    fn published_identity_binds_the_medium_and_its_marker() {
        let root = tempfile::tempdir().unwrap();
        assert!(card_marker(root.path()).is_err(), "no GRKIT folder");
        fs::create_dir(root.path().join("GRKIT")).unwrap();
        let marker = card_marker(root.path()).unwrap();
        assert_eq!(card_marker(root.path()).unwrap(), marker, "created once");
        let cid = "aa585951454d552101deadbeef006201";
        let id = public_card_id(cid, &marker).unwrap();
        // The CRC byte differs between the RTOS and Linux readings.
        assert_eq!(
            public_card_id("AA585951454D552101DEADBEEF0062FF", &marker).unwrap(),
            id
        );
        assert_ne!(
            public_card_id("ab585951454d552101deadbeef006201", &marker).unwrap(),
            id
        );
        // A format removes the marker: the next start makes a new identity.
        fs::remove_file(root.path().join(MARKER)).unwrap();
        let formatted = card_marker(root.path()).unwrap();
        assert_ne!(public_card_id(cid, &formatted).unwrap(), id);
        fs::write(root.path().join(MARKER), "not hex").unwrap();
        assert!(card_marker(root.path()).is_err());
        assert!(public_card_id("00000000000000000000000000000001", &marker).is_err());
    }
    #[test]
    fn unsupported_or_missing_identity_fails_closed() {
        assert!(card_id("unknown", &sector()).is_err());
        assert!(card_id("0123456789abcdef0123456789abcdef", &[0; 512]).is_err());
    }
}
