//! The signature of a GRKit release. For each camera, the release signs a
//! statement: the SHA-256 of each runtime file of a package for the camera,
//! and of each official module. A package that a phone builds holds the
//! statement and its signature, and the service on the camera installs a
//! package over Wi-Fi only when an official key signed its code. The effects
//! and the libraries are data, which the decoders check.
//!
//! The statement lists the runtime files by their paths on the card, so a
//! service checks the scripts of a later release by their hashes, not by its
//! own templates.

use std::collections::BTreeMap;

use anyhow::{bail, ensure, Context, Result};
use ed25519_compact::{KeyPair, PublicKey, Seed, Signature};
use serde_json::{json, Value};

use crate::{
    camera::{self, Camera},
    card,
    packaging::{self, DataCheck, VerifiedRuntime},
    Files,
};

/// The statement and the signature in a package ZIP, next to `SHA256SUMS`.
/// Copies on the card preserve the signature for runtime capability checks.
pub const STATEMENT_ENTRY: &str = "RELEASE.JSON";
pub const SIGNATURE_ENTRY: &str = "RELEASE.SIG";
pub const CARD_STATEMENT: &str = "SCRIPT/GRKIT/RELEASE.JSN";
pub const CARD_SIGNATURE: &str = "SCRIPT/GRKIT/RELEASE.SIG";

/// Card-side evidence must be an exact copy of the signed ZIP evidence.
/// It does not itself authorize any additional executable file.
pub(crate) fn check_card_evidence(name: &str, blob: &[u8], entries: &Files) -> Result<bool> {
    for (card, root) in [
        (CARD_STATEMENT, STATEMENT_ENTRY),
        (CARD_SIGNATURE, SIGNATURE_ENTRY),
    ] {
        if name == format!("SD_ROOT/{card}") {
            ensure!(
                entries.get(root).is_some_and(|original| original == blob),
                "Card release evidence differs from the signed package"
            );
            return Ok(true);
        }
    }
    Ok(false)
}
/// The version of the statement format.
const FORMAT: u64 = 1;

/// The public keys of the official releases. The private key of the first
/// one signs each release (`.github/workflows/release.yml`).
pub const OFFICIAL_KEYS: [[u8; 32]; 1] = [[
    0xc5, 0xcb, 0x2c, 0x21, 0xbb, 0x0a, 0xa8, 0x79, 0x32, 0xbc, 0xfc, 0xda, 0xc1, 0x5f, 0x7d, 0x20,
    0x2e, 0x00, 0x58, 0xc0, 0xc7, 0x4f, 0x38, 0x7c, 0x21, 0x12, 0x6f, 0xff, 0x39, 0xc4, 0x73, 0x92,
]];

/// What a release signs for one camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// The GRKit version, such as `1.7.0`.
    pub version: String,
    pub camera: &'static Camera,
    /// The SHA-256 of each runtime file, by its path on the card. A package
    /// has each of them.
    pub runtime: BTreeMap<String, String>,
    /// The SHA-256 of each official module file, such as `FILMLOOK.GRM`. A
    /// package can have any of them.
    pub modules: BTreeMap<String, String>,
    /// Trusted official GRF identity, independent of the files a user selected.
    pub official_looks: BTreeMap<String, String>,
    /// Data formats the signed target runtime can read. Absence proves nothing.
    pub data_formats: BTreeMap<String, u32>,
}

impl Statement {
    /// The statement for the package runtime of `camera` from the runtime of
    /// each of its builds, in release order, and the official module files
    /// `modules`.
    pub fn of_release(
        runtimes: &[VerifiedRuntime],
        modules: &[Vec<u8>],
        camera: &'static Camera,
    ) -> Result<Statement> {
        let runtime = packaging::package_runtime(runtimes, camera)?;
        let mut versions = runtimes
            .iter()
            .map(|runtime| packaging::marked_version(&runtime.files()[card::CORE_FILE]));
        let version = versions
            .next()
            .flatten()
            .context("The runtime does not name its GRKit version")?
            .to_owned();
        ensure!(
            versions.all(|other| other == Some(version.as_str())),
            "The runtimes of {} name different GRKit versions",
            camera.name
        );
        let mut files = BTreeMap::new();
        for bytes in modules {
            let id = crate::grm::Module::decode(bytes)?.id;
            ensure!(
                files
                    .insert(format!("{id}.GRM"), packaging::sha256(bytes))
                    .is_none(),
                "The module {id} is twice in the release"
            );
        }
        Ok(Statement {
            version,
            camera,
            runtime: runtime
                .iter()
                .map(|(path, bytes)| (path.clone(), packaging::sha256(bytes)))
                .collect(),
            modules: files,
            official_looks: BTreeMap::new(),
            data_formats: BTreeMap::from([("lookLibrary".into(), 1)]),
        })
    }

    /// The bytes that a release signs.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        packaging::json_bytes(&json!({
            "format": FORMAT,
            "version": self.version,
            "camera": self.camera.id,
            "runtime": self.runtime,
            "modules": self.modules,
            "officialLooks": self.official_looks,
            "dataFormats": self.data_formats,
        }))
    }

    /// Reads a statement. It does not check the signature (`verify`).
    pub fn parse(bytes: &[u8]) -> Result<Statement> {
        let value: Value =
            crate::effects::parse_json(bytes).context("Read the release statement")?;
        ensure!(
            value["format"] == FORMAT,
            "The release statement has another format"
        );
        let hashes = |key: &str| -> Result<BTreeMap<String, String>> {
            value[key]
                .as_object()
                .with_context(|| format!("The release statement has no {key} list"))?
                .iter()
                .map(|(path, hash)| {
                    let hash = hash
                        .as_str()
                        .filter(|hash| {
                            hash.len() == 64
                                && hash
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        })
                        .with_context(|| {
                            format!("The release statement has no SHA-256 for {path}")
                        })?;
                    Ok((path.clone(), hash.to_owned()))
                })
                .collect()
        };
        let official_looks = hashes("officialLooks")?;
        ensure!(
            official_looks.len() <= 256
                && official_looks.keys().all(|id| crate::effects::valid_id(id)),
            "Invalid official Look identities"
        );
        let data_formats: BTreeMap<String, u32> =
            serde_json::from_value(value["dataFormats"].clone())
                .context("Read supported data formats")?;
        ensure!(
            data_formats.len() <= 8
                && data_formats.iter().all(|(name, version)| matches!(
                    name.as_str(),
                    "lookLibrary" | "recipes" | "moduleSettings"
                ) && *version > 0),
            "Invalid supported data formats"
        );
        Ok(Statement {
            version: value["version"]
                .as_str()
                .context("The release statement has no version")?
                .to_owned(),
            camera: camera::find(
                value["camera"]
                    .as_str()
                    .context("The release statement has no camera")?,
            )?,
            runtime: hashes("runtime")?,
            modules: hashes("modules")?,
            official_looks,
            data_formats,
        })
    }
}

/// A signing key: the 32-byte seed, as 64 hexadecimal digits.
pub struct SigningKey(KeyPair);

impl SigningKey {
    pub fn from_hex(text: &str) -> Result<SigningKey> {
        let bytes = hex::decode(text.trim()).context("Read the signing key")?;
        let seed = Seed::from_slice(&bytes)
            .ok()
            .context("A signing key has 32 bytes")?;
        ensure!(bytes.iter().any(|&b| b != 0), "The signing key is empty");
        Ok(SigningKey(KeyPair::from_seed(seed)))
    }

    /// The public key, which `OFFICIAL_KEYS` holds for an official key.
    pub fn public_key(&self) -> [u8; 32] {
        *self.0.pk
    }

    /// The signature file of `statement`: 128 hexadecimal digits. Each
    /// signature of the same statement is the same.
    pub fn sign(&self, statement: &[u8]) -> Vec<u8> {
        format!("{}\n", hex::encode(*self.0.sk.sign(statement, None))).into_bytes()
    }
}

/// Reads `statement` when `signature` is the signature of one of `keys`.
pub fn verify(statement: &[u8], signature: &[u8], keys: &[[u8; 32]]) -> Result<Statement> {
    let text = std::str::from_utf8(signature).context("Read the release signature")?;
    let bytes = hex::decode(text.trim_end_matches('\n')).context("Read the release signature")?;
    let signature = Signature::from_slice(&bytes)
        .ok()
        .context("A release signature has 64 bytes")?;
    ensure!(
        keys.iter()
            .any(|key| PublicKey::new(*key).verify(statement, &signature).is_ok()),
        "No GRKit release key signed this package"
    );
    Statement::parse(statement)
}

/// What a signed update holds.
#[derive(Debug)]
pub struct Update {
    pub statement: Statement,
    pub effect_ids: Vec<String>,
    pub modules: Vec<String>,
}

/// Checks the files of a package ZIP `entries` against `statement`: each
/// runtime file of the statement, with its hash; modules of the statement;
/// and data that the decoders check. The update puts no other file on the
/// card. The files outside `SD_ROOT/SCRIPT`, such as the ModeSet file and
/// the guide, are not part of the update.
pub fn check_files(entries: &Files, statement: &Statement) -> Result<Update> {
    let mut data = DataCheck::new(statement.camera);
    let mut runtime = 0;
    for (name, blob) in entries {
        let Some(path) = name.strip_prefix("SD_ROOT/") else {
            continue;
        };
        if check_card_evidence(name, blob, entries)? {
            continue;
        }
        if !path.starts_with("SCRIPT/") || path == crate::card::PACKAGE_LIST {
            continue;
        }
        if let Some(hash) = statement.runtime.get(path) {
            ensure!(
                *hash == packaging::sha256(blob),
                "{path} is not the file that the release signed"
            );
            runtime += 1;
            continue;
        }
        if let Some(file) = path
            .strip_prefix(crate::grm::FOLDER)
            .and_then(|path| path.strip_prefix('/'))
        {
            ensure!(
                statement.modules.get(file) == Some(&packaging::sha256(blob)),
                "The module {file} is not an official module of the release"
            );
        }
        if !data.check(name, blob)? {
            bail!("The release did not sign {path}");
        }
    }
    ensure!(
        runtime == statement.runtime.len(),
        "The package does not have each runtime file of the release"
    );
    let list = entries
        .get(&format!("SD_ROOT/{}", crate::card::PACKAGE_LIST))
        .context("The package has no package list")?;
    let data = data.finish(list)?;
    Ok(Update {
        statement: statement.clone(),
        effect_ids: data.effect_ids,
        modules: data.modules,
    })
}

/// Checks a package ZIP for an update over Wi-Fi: its entries and
/// `SHA256SUMS`, the signature of one of `keys`, and its files
/// (`check_files`). Returns what it holds, and the files of the update at
/// their paths from the card root.
pub fn verify_update(zip: &[u8], keys: &[[u8; 32]]) -> Result<(Update, Files)> {
    let entries = packaging::read_entries(zip)?;
    ensure!(
        entries.get("SHA256SUMS") == Some(&packaging::checksum_list(&entries)),
        "SHA256SUMS differs from package"
    );
    let entry = |name: &str| {
        entries
            .get(name)
            .with_context(|| format!("The package is not signed: it has no {name}"))
    };
    let statement = verify(entry(STATEMENT_ENTRY)?, entry(SIGNATURE_ENTRY)?, keys)?;
    let update = check_files(&entries, &statement)?;
    let files = entries
        .into_iter()
        .filter_map(|(name, blob)| {
            name.strip_prefix("SD_ROOT/")
                .filter(|path| path.starts_with("SCRIPT/"))
                .map(|path| (path.to_owned(), blob))
        })
        .collect();
    Ok((update, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "0101010101010101010101010101010101010101010101010101010101010101";

    #[test]
    fn a_signature_verifies_only_with_its_key_and_statement() {
        let key = SigningKey::from_hex(SEED).unwrap();
        let statement = Statement {
            version: "1.7.0".into(),
            camera: camera::find("gr3").unwrap(),
            runtime: BTreeMap::from([(card::CORE_FILE.to_owned(), "ab".repeat(32))]),
            modules: BTreeMap::new(),
            official_looks: BTreeMap::new(),
            data_formats: BTreeMap::new(),
        };
        let bytes = statement.to_bytes().unwrap();
        let signature = key.sign(&bytes);
        assert_eq!(signature, key.sign(&bytes));
        assert_eq!(
            verify(&bytes, &signature, &[key.public_key()]).unwrap(),
            statement
        );
        assert!(verify(&bytes, &signature, &OFFICIAL_KEYS).is_err());
        let mut other = bytes.clone();
        other[10] ^= 1;
        assert!(verify(&other, &signature, &[key.public_key()]).is_err());
    }

    #[test]
    fn official_look_and_format_claims_are_bound_to_the_signature() {
        let key = SigningKey::from_hex(SEED).unwrap();
        let mut statement = Statement::of_release(
            &crate::testing::release_runtimes(&camera::GR3, "1.7.0"),
            &[],
            &camera::GR3,
        )
        .unwrap();
        statement.official_looks.insert(
            "official".into(),
            packaging::sha256(&crate::testing::effect("official")),
        );
        assert_eq!(statement.data_formats.get("lookLibrary"), Some(&1));
        let bytes = statement.to_bytes().unwrap();
        let signature = key.sign(&bytes);
        let verified = verify(&bytes, &signature, &[key.public_key()]).unwrap();
        assert_eq!(verified.data_formats["lookLibrary"], 1);
        assert_eq!(verified.official_looks, statement.official_looks);
        statement.data_formats.insert("lookLibrary".into(), 2);
        assert!(verify(
            &statement.to_bytes().unwrap(),
            &signature,
            &[key.public_key()]
        )
        .is_err());
    }

    /// The entries of a signed package of the tests, and its statement.
    fn signed() -> (Files, Statement) {
        let zip = crate::testing::signed_package(&camera::GR3, "1.7.0", &["test-effect"]);
        let (update, _) = verify_update(&zip, &[crate::testing::release_key()]).unwrap();
        (packaging::read_entries(&zip).unwrap(), update.statement)
    }

    #[test]
    fn an_update_has_the_signed_files_of_the_card() {
        let zip = crate::testing::signed_package(&camera::GR3, "1.7.0", &["test-effect"]);
        let (update, files) = verify_update(&zip, &[crate::testing::release_key()]).unwrap();
        assert_eq!(update.statement.version, "1.7.0");
        assert_eq!(update.statement.camera, &camera::GR3);
        assert_eq!(update.effect_ids.len(), 1);
        assert!(files.keys().all(|path| path.starts_with("SCRIPT/")));
        assert!(update
            .statement
            .runtime
            .keys()
            .all(|path| files.contains_key(path)));
        assert!(files.contains_key("SCRIPT/STARTUP.TTL"));
        assert!(verify_update(&zip, &OFFICIAL_KEYS).is_err());
    }

    #[test]
    fn an_update_has_only_the_code_that_the_release_signed() {
        let (entries, statement) = signed();
        let error = |entries: &Files, statement: &Statement| {
            format!("{:#}", check_files(entries, statement).unwrap_err())
        };
        let mut changed = entries.clone();
        changed
            .get_mut("SD_ROOT/SCRIPT/GRKIT/SCAN.SH")
            .unwrap()
            .push(b'\n');
        assert!(error(&changed, &statement).contains("not the file that the release signed"));
        let mut missing = entries.clone();
        missing.remove("SD_ROOT/SCRIPT/GRKIT/SCAN.SH");
        assert!(error(&missing, &statement).contains("each runtime file"));
        let mut extra = entries.clone();
        extra.insert("SD_ROOT/SCRIPT/GRKIT/EXTRA.BIN".into(), vec![0; 4]);
        assert!(error(&extra, &statement).contains("did not sign SCRIPT/GRKIT/EXTRA.BIN"));
        let mut module = entries.clone();
        let (path, _) = entries
            .iter()
            .find(|(path, _)| path.ends_with(".GRM"))
            .unwrap();
        let id = path.rsplit('/').next().unwrap().trim_end_matches(".GRM");
        let mut other = crate::grm::Module::decode(&entries[path]).unwrap();
        other.version += 1;
        module.insert(path.clone(), other.encode().unwrap());
        assert!(error(&module, &statement).contains(&format!("{id}.GRM is not an official module")));
    }

    #[test]
    fn a_later_release_can_sign_files_that_this_version_does_not_know() {
        let (mut entries, mut statement) = signed();
        let script = b"; a script of a later release\n".to_vec();
        statement
            .runtime
            .insert("SCRIPT/GRKIT/LATER.TTL".into(), packaging::sha256(&script));
        entries.insert("SD_ROOT/SCRIPT/GRKIT/LATER.TTL".into(), script);
        let changed = entries.get_mut("SD_ROOT/SCRIPT/GRKIT/LOADER.TTL").unwrap();
        changed.extend_from_slice(b"; changed\n");
        let hash = packaging::sha256(changed);
        statement
            .runtime
            .insert("SCRIPT/GRKIT/LOADER.TTL".into(), hash);
        check_files(&entries, &statement).unwrap();
    }

    #[test]
    fn a_signing_key_has_32_bytes() {
        assert!(SigningKey::from_hex("00").is_err());
        assert!(SigningKey::from_hex(&"00".repeat(32)).is_err());
        assert!(SigningKey::from_hex(&format!("{SEED}\n")).is_ok());
    }
}
