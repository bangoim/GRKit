//! Managed Look configuration. Preparing a candidate never writes the card or
//! changes camera memory. The coordinator must recheck its revision, commit via
//! the RTOS storage owner, and wait for runtime acknowledgement before publishing.
use crate::protocol::{strong_etag, Problem};
use grkit_core::{abi::GRF_FEATURES, effects, packaging};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const FORMAT_VERSION: u32 = 1;
pub const MAX_LISTED: usize = 64;
pub const MAX_LOADED: usize = 40;
/// Includes removed identities. Never discard an official tombstone to free space.
pub const MAX_IDENTITIES: usize = 256;
pub const MAX_MANIFEST_BYTES: usize = 128 * 1024;

/// RTOS storage representation. All integers are little-endian.
pub const BINARY_MAGIC: &[u8; 8] = b"GRKLIB01";
pub const BINARY_HEADER_SIZE: usize = 32;
pub const BINARY_ENTRY_SIZE: usize = 96;
pub const MAX_BINARY_BYTES: usize =
    BINARY_HEADER_SIZE + MAX_IDENTITIES * BINARY_ENTRY_SIZE + MAX_LISTED * 2;
const BINARY_CRC_OFFSET: usize = 24;
const ENTRY_HASH_OFFSET: usize = 52;
const ENTRY_FLAGS_OFFSET: usize = 84;
const ENTRY_FEATURES_OFFSET: usize = 88;

type Result<T> = std::result::Result<T, Problem>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    Official,
    Custom,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub content_hash: String,
    pub enabled: bool,
    pub origin: Origin,
    pub removed: bool,
    pub features: u32,
    pub monochrome: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub entries: BTreeMap<String, Entry>,
    pub order: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub content_hash: &'a str,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
}

impl Patch {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 1024 {
            return Err(problem(
                413,
                "patch_too_large",
                "The Look patch exceeds its size limit.",
            ));
        }
        let value = effects::parse_json(bytes)
            .map_err(|_| problem(422, "invalid_patch", "The Look patch is invalid."))?;
        let fields = value
            .as_object()
            .ok_or_else(|| problem(422, "invalid_patch", "The Look patch must be an object."))?;
        if fields.is_empty()
            || fields.iter().any(|(key, value)| match key.as_str() {
                "name" => !value.is_string(),
                "enabled" => !value.is_boolean(),
                _ => true,
            })
        {
            return Err(problem(
                422,
                "invalid_patch",
                "Supply only a name string or an enabled boolean.",
            ));
        }
        serde_json::from_value(value)
            .map_err(|_| problem(422, "invalid_patch", "The Look patch is invalid."))
    }
}

/// Inputs come from a coherent runtime snapshot, not from a mutation body.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The owner has admitted the complete hot-reload commit protocol.
    pub hot_reload: bool,
    pub selected: Option<String>,
    /// True only when recipes AND dial-set dependencies were read successfully.
    pub dependencies_complete: bool,
    pub dependencies: BTreeMap<String, Vec<String>>,
    /// Duplicate files and unresolved runtime code mappings block affected IDs.
    pub blocked_ids: BTreeSet<String>,
    pub reserved_codes: BTreeSet<u16>,
    pub supported_features: u32,
    pub monochrome_only: bool,
    /// Verified catalog or signed package contents, keyed by stable Look ID.
    pub official_hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub id: String,
    pub hash: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub manifest: Manifest,
    /// The RTOS owner stores this immutable content generation before publication.
    pub content: Option<Content>,
    /// Apply under the same guard as publishing the new library.
    pub select_off: bool,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            entries: BTreeMap::new(),
            order: Vec::new(),
        }
    }
}

impl Manifest {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(problem(
                413,
                "manifest_too_large",
                "The managed library exceeds its storage limit.",
            ));
        }
        // The generic serde map parser otherwise accepts repeated object keys.
        let value = effects::parse_json(bytes).map_err(|_| invalid_manifest())?;
        let manifest: Self = serde_json::from_value(value).map_err(|_| invalid_manifest())?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Encode the RTOS-owned on-card representation. The header contains magic,
    /// format, byte length, entry count, order count, CRC, and a reserved word.
    /// Each 96-byte entry contains ID[32], name[20], SHA-256[32], flags, features,
    /// and a reserved word. The tail contains ordered u16 entry indices. Indices
    /// name serialized records, never runtime settings handles or memory slots.
    pub fn encode_binary(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let length =
            BINARY_HEADER_SIZE + self.entries.len() * BINARY_ENTRY_SIZE + self.order.len() * 2;
        let mut bytes = vec![0; length];
        bytes[..8].copy_from_slice(BINARY_MAGIC);
        put_word(&mut bytes, 8, self.format_version);
        put_word(&mut bytes, 12, length as u32);
        put_word(&mut bytes, 16, self.entries.len() as u32);
        put_word(&mut bytes, 20, self.order.len() as u32);
        let mut indices = BTreeMap::new();
        for (index, (id, entry)) in self.entries.iter().enumerate() {
            indices.insert(id.as_str(), index as u16);
            let start = BINARY_HEADER_SIZE + index * BINARY_ENTRY_SIZE;
            let record = &mut bytes[start..start + BINARY_ENTRY_SIZE];
            record[..id.len()].copy_from_slice(id.as_bytes());
            record[32..32 + entry.name.len()].copy_from_slice(entry.name.as_bytes());
            for (i, pair) in entry
                .content_hash
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
            {
                // validate() already established exactly 64 lowercase hex digits.
                record[ENTRY_HASH_OFFSET + i] = hex_nibble(pair[0]) * 16 + hex_nibble(pair[1]);
            }
            let origin = match entry.origin {
                Origin::Unknown => 0,
                Origin::Custom => 1,
                Origin::Official => 2,
            };
            let flags = u32::from(entry.enabled)
                | (u32::from(entry.removed) << 1)
                | (u32::from(entry.monochrome) << 2)
                | (origin << 3);
            put_word(record, ENTRY_FLAGS_OFFSET, flags);
            put_word(record, ENTRY_FEATURES_OFFSET, entry.features);
        }
        let order_start = BINARY_HEADER_SIZE + self.entries.len() * BINARY_ENTRY_SIZE;
        for (position, id) in self.order.iter().enumerate() {
            let index = indices[id.as_str()];
            let start = order_start + position * 2;
            bytes[start..start + 2].copy_from_slice(&index.to_le_bytes());
        }
        let crc = binary_crc(&bytes);
        put_word(&mut bytes, BINARY_CRC_OFFSET, crc);
        Ok(bytes)
    }

    /// Reject noncanonical padding, unknown flags, duplicate IDs, invalid order,
    /// and unsupported formats before exposing any configuration to the caller.
    pub fn decode_binary(bytes: &[u8]) -> Result<Self> {
        if !(BINARY_HEADER_SIZE..=MAX_BINARY_BYTES).contains(&bytes.len()) {
            return Err(invalid_manifest());
        }
        if &bytes[..8] != BINARY_MAGIC {
            return Err(invalid_manifest());
        }
        let format_version = word(bytes, 8);
        if format_version != FORMAT_VERSION {
            return Err(problem(
                409,
                "data_format_incompatible",
                "The runtime does not support this library format.",
            ));
        }
        let count = word(bytes, 16) as usize;
        let order_count = word(bytes, 20) as usize;
        if count > MAX_IDENTITIES
            || order_count > MAX_LISTED
            || word(bytes, 12) as usize != bytes.len()
            || bytes.len() != BINARY_HEADER_SIZE + count * BINARY_ENTRY_SIZE + order_count * 2
            || word(bytes, 28) != 0
            || word(bytes, BINARY_CRC_OFFSET) != binary_crc(bytes)
        {
            return Err(invalid_manifest());
        }
        let mut entries = BTreeMap::new();
        let mut ids = Vec::with_capacity(count);
        for index in 0..count {
            let start = BINARY_HEADER_SIZE + index * BINARY_ENTRY_SIZE;
            let record = &bytes[start..start + BINARY_ENTRY_SIZE];
            let id = padded_text(&record[..32])?;
            let name = padded_text(&record[32..ENTRY_HASH_OFFSET])?;
            if ids
                .last()
                .is_some_and(|previous: &String| previous.as_str() >= id)
            {
                return Err(invalid_manifest());
            }
            let flags = word(record, ENTRY_FLAGS_OFFSET);
            if flags & !0x1f != 0 || word(record, 92) != 0 {
                return Err(invalid_manifest());
            }
            let origin = match (flags >> 3) & 3 {
                0 => Origin::Unknown,
                1 => Origin::Custom,
                2 => Origin::Official,
                _ => return Err(invalid_manifest()),
            };
            let content_hash: String = record[ENTRY_HASH_OFFSET..ENTRY_FLAGS_OFFSET]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            entries.insert(
                id.to_owned(),
                Entry {
                    name: name.to_owned(),
                    content_hash,
                    enabled: flags & 1 != 0,
                    removed: flags & 2 != 0,
                    monochrome: flags & 4 != 0,
                    origin,
                    features: word(record, ENTRY_FEATURES_OFFSET),
                },
            );
            ids.push(id.to_owned());
        }
        let order_start = BINARY_HEADER_SIZE + count * BINARY_ENTRY_SIZE;
        let mut order = Vec::with_capacity(order_count);
        for position in 0..order_count {
            let start = order_start + position * 2;
            let index =
                u16::from_le_bytes(bytes[start..start + 2].try_into().expect("bounded order"))
                    as usize;
            order.push(ids.get(index).ok_or_else(invalid_manifest)?.clone());
        }
        let manifest = Self {
            format_version,
            entries,
            order,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).expect("manifest serializes");
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(problem(
                413,
                "manifest_too_large",
                "The managed library exceeds its storage limit.",
            ));
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format_version != FORMAT_VERSION {
            return Err(problem(
                409,
                "data_format_incompatible",
                "The runtime does not support this library format.",
            ));
        }
        if self.entries.len() > MAX_IDENTITIES || self.order.len() > MAX_LISTED {
            return Err(capacity());
        }
        let active: BTreeSet<_> = self
            .entries
            .iter()
            .filter(|(_, e)| !e.removed)
            .map(|(id, _)| id)
            .collect();
        let ordered: BTreeSet<_> = self.order.iter().collect();
        if active != ordered || ordered.len() != self.order.len() {
            return Err(invalid_manifest());
        }
        for (id, entry) in &self.entries {
            if !effects::valid_id(id)
                || !valid_name(&entry.name)
                || !valid_hash(&entry.content_hash)
                || (entry.removed && entry.enabled)
            {
                return Err(invalid_manifest());
            }
        }
        if self
            .entries
            .values()
            .filter(|e| !e.removed && e.enabled)
            .count()
            > MAX_LOADED
        {
            return Err(capacity());
        }
        Ok(())
    }

    /// Resolve legacy Unknown entries using verified catalog content. Stored
    /// official tombstones keep their identity when the current catalog changes.
    pub fn origin(&self, id: &str, context: &Context) -> Result<Origin> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| problem(404, "look_not_found", "The Look does not exist."))?;
        Ok(
            if entry.origin == Origin::Unknown
                && context.official_hashes.get(id) == Some(&entry.content_hash)
            {
                Origin::Official
            } else {
                entry.origin
            },
        )
    }

    pub fn configuration<'a>(&'a self, id: &'a str) -> Result<Configuration<'a>> {
        let entry = self.active(id)?;
        Ok(Configuration {
            id,
            name: &entry.name,
            content_hash: &entry.content_hash,
            enabled: entry.enabled,
        })
    }

    pub fn configuration_bytes(&self, id: &str) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.configuration(id)?).expect("configuration serializes"))
    }

    pub fn configuration_etag(&self, id: &str) -> Result<String> {
        Ok(strong_etag(&self.configuration_bytes(id)?))
    }

    pub fn order_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"ids": self.order})).expect("order serializes")
    }

    pub fn order_etag(&self) -> String {
        strong_etag(&self.order_bytes())
    }

    /// The caller separately checks If-None-Match/If-Match for this content URI.
    pub fn prepare_put(&self, id: &str, bytes: &[u8], context: &Context) -> Result<Prepared> {
        self.validate()?;
        context.check_id(id)?;
        let (effect, _) = decode_content(id, bytes, context)?;
        let hash = packaging::sha256(bytes);
        let previous = self.entries.get(id);
        let verified = context
            .official_hashes
            .get(id)
            .is_some_and(|expected| expected == &hash);
        let previous_origin = previous.map(|_| self.origin(id, context)).transpose()?;
        let origin = match previous_origin {
            Some(Origin::Official) => {
                // Restoring a tombstone still requires trusted content. An active
                // official Look can only be refreshed by package reconciliation.
                if previous.is_some_and(|entry| !entry.removed) || !verified {
                    return Err(official_read_only());
                }
                Origin::Official
            }
            Some(Origin::Unknown) => return Err(origin_unknown()),
            Some(Origin::Custom) => {
                if context.official_hashes.contains_key(id) {
                    return Err(problem(
                        409,
                        "origin_conflict",
                        "The custom ID conflicts with a trusted official Look.",
                    ));
                }
                Origin::Custom
            }
            None => {
                if context.official_hashes.contains_key(id) && !verified {
                    return Err(official_read_only());
                }
                if verified {
                    Origin::Official
                } else {
                    Origin::Custom
                }
            }
        };
        self.check_collision(id, context)?;
        let mut candidate = self.clone();
        let existed = previous.is_some_and(|entry| !entry.removed);
        let enabled = previous
            .filter(|entry| !entry.removed)
            .is_none_or(|entry| entry.enabled);
        let monochrome = effect.monochrome();
        candidate.entries.insert(
            id.to_owned(),
            Entry {
                name: effect.name,
                content_hash: hash.clone(),
                enabled,
                origin,
                removed: false,
                features: features(bytes),
                monochrome,
            },
        );
        if !existed {
            candidate.order.push(id.to_owned());
        }
        candidate.validate()?;
        Ok(Prepared {
            manifest: candidate,
            content: Some(Content {
                id: id.to_owned(),
                hash,
                bytes: bytes.to_vec(),
            }),
            select_off: false,
        })
    }

    /// `current_content` must be the generation named by the current manifest.
    pub fn prepare_patch(
        &self,
        id: &str,
        patch: &Patch,
        current_content: &[u8],
        context: &Context,
    ) -> Result<Prepared> {
        self.validate()?;
        context.check_id(id)?;
        let entry = self.active(id)?;
        if patch.name.is_none() && patch.enabled.is_none() {
            return Err(problem(422, "invalid_patch", "Supply name or enabled."));
        }
        let origin = self.origin(id, context)?;
        if patch.name.is_some() {
            if entry.origin == Origin::Custom && context.official_hashes.contains_key(id) {
                return Err(problem(
                    409,
                    "origin_conflict",
                    "The custom ID conflicts with a trusted official Look.",
                ));
            }
            match origin {
                Origin::Official => return Err(official_read_only()),
                Origin::Unknown => return Err(origin_unknown()),
                Origin::Custom => {}
            }
        }
        if packaging::sha256(current_content) != entry.content_hash {
            return Err(problem(
                503,
                "content_unavailable",
                "The current content generation does not match the manifest.",
            ));
        }
        let (mut effect, icon) = decode_content(id, current_content, context)?;
        if effect.name != entry.name
            || features(current_content) != entry.features
            || effect.monochrome() != entry.monochrome
        {
            return Err(problem(
                503,
                "content_unavailable",
                "The content metadata does not match the manifest.",
            ));
        }
        self.check_collision(id, context)?;
        let mut candidate = self.clone();
        let updated = candidate.entries.get_mut(id).expect("active entry exists");
        updated.origin = origin;
        if let Some(enabled) = patch.enabled {
            updated.enabled = enabled;
        }
        let content = if let Some(name) = &patch.name {
            if !valid_name(name) {
                return Err(problem(
                    422,
                    "invalid_name",
                    "Use 1 to 16 printable ASCII characters.",
                ));
            }
            effect.name.clone_from(name);
            let bytes = effect
                .encode(&icon)
                .map_err(|_| problem(422, "invalid_grf", "The renamed Look cannot be encoded."))?;
            updated.name.clone_from(name);
            updated.content_hash = packaging::sha256(&bytes);
            Some(Content {
                id: id.to_owned(),
                hash: updated.content_hash.clone(),
                bytes,
            })
        } else {
            None
        };
        candidate.validate()?;
        Ok(Prepared {
            manifest: candidate,
            content,
            select_off: false,
        })
    }

    pub fn prepare_delete(
        &self,
        id: &str,
        active_off: bool,
        context: &Context,
    ) -> Result<Prepared> {
        self.validate()?;
        context.check_id(id)?;
        self.active(id)?;
        if !context.dependencies_complete {
            return Err(problem(
                503,
                "dependencies_unavailable",
                "Recipe and dial-set references must be resolved before removal.",
            ));
        }
        if context
            .dependencies
            .get(id)
            .is_some_and(|refs| !refs.is_empty())
        {
            return Err(problem(
                409,
                "dependency_in_use",
                "A recipe or dial set references this Look.",
            ));
        }
        let selected = context.selected.as_deref() == Some(id);
        if selected && !active_off {
            return Err(problem(
                409,
                "active_look",
                "Removing the selected Look requires active=off.",
            ));
        }
        let mut candidate = self.clone();
        let removed = candidate.entries.get_mut(id).expect("active entry exists");
        removed.origin = self.origin(id, context)?;
        removed.removed = true;
        removed.enabled = false;
        candidate.order.retain(|entry| entry != id);
        candidate.validate()?;
        Ok(Prepared {
            manifest: candidate,
            content: None,
            select_off: selected,
        })
    }

    pub fn prepare_order(&self, ids: &[String], context: &Context) -> Result<Prepared> {
        self.validate()?;
        if ids.len() > MAX_LISTED {
            return Err(problem(
                422,
                "invalid_order",
                "List each active Look ID exactly once.",
            ));
        }
        for id in &self.order {
            context.check_id(id)?;
        }
        let mut candidate = self.clone();
        candidate.order = ids.to_vec();
        candidate.validate().map_err(|_| {
            problem(
                422,
                "invalid_order",
                "List each active Look ID exactly once.",
            )
        })?;
        Ok(Prepared {
            manifest: candidate,
            content: None,
            select_off: false,
        })
    }

    fn active(&self, id: &str) -> Result<&Entry> {
        self.entries
            .get(id)
            .filter(|entry| !entry.removed)
            .ok_or_else(|| {
                problem(
                    404,
                    "look_not_found",
                    "The Look is not in the managed library.",
                )
            })
    }

    fn check_collision(&self, id: &str, context: &Context) -> Result<()> {
        let code = effect_code(id);
        // Keep removed identities reserved. Reuse could make restoration select
        // another Look through existing compact recipe or persisted references.
        // Reserve the native GR IV page on every camera so a managed Look ID
        // remains portable without assigning a different code on import.
        if code >> 8 == 0x81
            || context.reserved_codes.contains(&code)
            || self
                .entries
                .keys()
                .any(|other| other != id && effect_code(other) == code)
        {
            return Err(problem(
                409,
                "effect_code_collision",
                "Choose another Look ID because its runtime code is already reserved.",
            ));
        }
        Ok(())
    }
}

impl Context {
    fn check_id(&self, id: &str) -> Result<()> {
        if !effects::valid_id(id) {
            return Err(problem(422, "invalid_id", "The Look ID is invalid."));
        }
        if self.blocked_ids.contains(id) {
            return Err(problem(409, "ambiguous_look", "Resolve duplicate files or ambiguous runtime references before changing this Look."));
        }
        Ok(())
    }
}

fn decode_content(id: &str, bytes: &[u8], context: &Context) -> Result<(effects::Effect, Vec<u8>)> {
    if bytes.len() != effects::SIZE && bytes.len() != effects::SIZE_WITH_LEVELS {
        return Err(problem(422, "invalid_grf", "The GRF length is invalid."));
    }
    let (effect, icon) = effects::decode(bytes)
        .map_err(|_| problem(422, "invalid_grf", "The GRF content is invalid."))?;
    if effect.id != id {
        return Err(problem(
            422,
            "id_mismatch",
            "The GRF ID does not match the resource ID.",
        ));
    }
    if features(bytes) & !context.supported_features != 0
        || (context.monochrome_only && !effect.monochrome())
    {
        return Err(problem(
            422,
            "unsupported_look",
            "The target runtime does not support this Look.",
        ));
    }
    Ok((effect, icon))
}
// CRC-32/ISO-HDLC (IEEE), with the checksum word treated as zero. The C reader
// uses the same polynomial and bounds; this checksum detects torn/corrupt data,
// but does not authenticate a manifest or establish transaction commit order.
fn binary_crc(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for (index, byte) in bytes.iter().enumerate() {
        let byte = if (BINARY_CRC_OFFSET..BINARY_CRC_OFFSET + 4).contains(&index) {
            0
        } else {
            *byte
        };
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("bounded word"))
}
fn put_word(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn padded_text(bytes: &[u8]) -> Result<&str> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(invalid_manifest)?;
    if bytes[end..].iter().any(|byte| *byte != 0) {
        return Err(invalid_manifest());
    }
    std::str::from_utf8(&bytes[..end]).map_err(|_| invalid_manifest())
}
fn hex_nibble(byte: u8) -> u8 {
    if byte <= b'9' {
        byte - b'0'
    } else {
        byte - b'a' + 10
    }
}

fn features(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(
        bytes[GRF_FEATURES..GRF_FEATURES + 4]
            .try_into()
            .expect("validated GRF"),
    )
}
fn effect_code(id: &str) -> u16 {
    0x8000
        | (id.bytes().fold(2166136261u32, |h, byte| {
            (h ^ u32::from(byte)).wrapping_mul(16777619)
        }) as u16
            & 0x7fff)
}
fn valid_name(name: &str) -> bool {
    (1..=effects::NAME_MAX).contains(&name.len()) && name.bytes().all(|b| (32..=126).contains(&b))
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn problem(status: u16, code: &'static str, detail: &'static str) -> Problem {
    Problem::new(status, code, detail)
}
fn invalid_manifest() -> Problem {
    problem(
        422,
        "invalid_manifest",
        "The managed library manifest is invalid.",
    )
}
fn official_read_only() -> Problem {
    problem(
        409,
        "official_look_read_only",
        "Create a custom copy to change official Look content.",
    )
}
fn origin_unknown() -> Problem {
    problem(
        409,
        "origin_unknown",
        "Resolve the Look origin before replacing its content.",
    )
}
fn capacity() -> Problem {
    problem(
        409,
        "look_capacity",
        "The Look count or retained identity count exceeds the supported capacity.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use grkit_core::testing;

    fn context() -> Context {
        Context {
            supported_features: u32::MAX,
            dependencies_complete: true,
            ..Default::default()
        }
    }
    fn one(id: &str) -> Manifest {
        Manifest::default()
            .prepare_put(id, &testing::effect(id), &context())
            .unwrap()
            .manifest
    }

    fn refresh_crc(bytes: &mut [u8]) {
        let crc = binary_crc(bytes);
        put_word(bytes, BINARY_CRC_OFFSET, crc);
    }

    #[test]
    fn binary_manifest_has_a_canonical_small_layout_and_preserves_origins() {
        let empty = Manifest::default().encode_binary().unwrap();
        assert_eq!(empty.len(), 32);
        // Independently calculated using Python zlib.crc32 on the header with
        // its checksum word zero: catches changes to layout or CRC convention.
        assert_eq!(word(&empty, BINARY_CRC_OFFSET), 0x2e742463);
        let mut manifest = one("a");
        for (id, origin, removed) in [("b", Origin::Official, true), ("c", Origin::Unknown, false)]
        {
            let mut entry = manifest.entries["a"].clone();
            entry.origin = origin;
            entry.removed = removed;
            entry.enabled = !removed;
            entry.monochrome = true;
            manifest.entries.insert(id.into(), entry);
            if !removed {
                manifest.order.insert(0, id.into());
            }
        }
        let bytes = manifest.encode_binary().unwrap();
        assert_eq!(bytes.len(), BINARY_HEADER_SIZE + 3 * BINARY_ENTRY_SIZE + 4);
        let decoded = Manifest::decode_binary(&bytes).unwrap();
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.encode_binary().unwrap(), bytes);
        assert_eq!(MAX_BINARY_BYTES, 24736);
    }

    #[test]
    fn binary_manifest_rejects_truncation_corruption_and_unknown_format() {
        let bytes = one("sample").encode_binary().unwrap();
        for end in 0..bytes.len() {
            assert!(
                Manifest::decode_binary(&bytes[..end]).is_err(),
                "accepted truncated length {end}"
            );
        }
        let mut corrupt = bytes.clone();
        corrupt[BINARY_HEADER_SIZE + ENTRY_HASH_OFFSET] ^= 1;
        assert!(Manifest::decode_binary(&corrupt).is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Manifest::decode_binary(&extra).is_err());
        let mut future = bytes.clone();
        put_word(&mut future, 8, FORMAT_VERSION + 1);
        refresh_crc(&mut future);
        assert_eq!(
            Manifest::decode_binary(&future).unwrap_err().code,
            "data_format_incompatible"
        );
        let mut excessive = bytes;
        put_word(&mut excessive, 16, u32::MAX);
        refresh_crc(&mut excessive);
        assert!(Manifest::decode_binary(&excessive).is_err());
    }

    #[test]
    fn binary_manifest_rejects_duplicate_ids_and_invalid_order_even_with_valid_crc() {
        let manifest = one("a")
            .prepare_put("b", &testing::effect("b"), &context())
            .unwrap()
            .manifest;
        let bytes = manifest.encode_binary().unwrap();
        let mut duplicate = bytes.clone();
        let id = duplicate[BINARY_HEADER_SIZE..BINARY_HEADER_SIZE + 32].to_vec();
        duplicate
            [BINARY_HEADER_SIZE + BINARY_ENTRY_SIZE..BINARY_HEADER_SIZE + BINARY_ENTRY_SIZE + 32]
            .copy_from_slice(&id);
        refresh_crc(&mut duplicate);
        assert!(Manifest::decode_binary(&duplicate).is_err());
        let order_start = BINARY_HEADER_SIZE + 2 * BINARY_ENTRY_SIZE;
        for indices in [[0u16, 0u16], [0u16, 2u16]] {
            let mut invalid = bytes.clone();
            for (i, index) in indices.iter().enumerate() {
                invalid[order_start + 2 * i..order_start + 2 * i + 2]
                    .copy_from_slice(&index.to_le_bytes());
            }
            refresh_crc(&mut invalid);
            assert!(Manifest::decode_binary(&invalid).is_err());
        }
        let mut removed_in_order = bytes;
        put_word(
            &mut removed_in_order,
            BINARY_HEADER_SIZE + ENTRY_FLAGS_OFFSET,
            (1 << 3) | 2,
        );
        refresh_crc(&mut removed_in_order);
        assert!(Manifest::decode_binary(&removed_in_order).is_err());
    }

    #[test]
    fn binary_manifest_rejects_reserved_bits_and_nonzero_text_padding() {
        let bytes = one("a").encode_binary().unwrap();
        for at in [
            28,
            BINARY_HEADER_SIZE + 2,
            BINARY_HEADER_SIZE + 51,
            BINARY_HEADER_SIZE + 92,
        ] {
            let mut invalid = bytes.clone();
            invalid[at] = 1;
            refresh_crc(&mut invalid);
            assert!(
                Manifest::decode_binary(&invalid).is_err(),
                "accepted nonzero padding {at}"
            );
        }
        for flags in [32, 3 << 3] {
            let mut invalid = bytes.clone();
            put_word(&mut invalid, BINARY_HEADER_SIZE + ENTRY_FLAGS_OFFSET, flags);
            refresh_crc(&mut invalid);
            assert!(Manifest::decode_binary(&invalid).is_err());
        }
    }

    #[test]
    fn content_creation_validates_identity_and_preserves_source_state() {
        let initial = Manifest::default();
        let prepared = initial
            .prepare_put("requests", &testing::effect("requests"), &context())
            .unwrap();
        assert!(initial.entries.is_empty());
        assert_eq!(prepared.manifest.order, ["requests"]);
        assert!(prepared.manifest.entries["requests"].enabled);
        assert_eq!(prepared.manifest.entries["requests"].origin, Origin::Custom);
        assert_eq!(
            prepared.content.unwrap().hash,
            packaging::sha256(&testing::effect("requests"))
        );
        assert_eq!(
            initial
                .prepare_put("order", &testing::effect("different"), &context())
                .unwrap_err()
                .code,
            "id_mismatch"
        );
        assert_eq!(
            initial
                .prepare_put("../bad", b"x", &context())
                .unwrap_err()
                .code,
            "invalid_id"
        );
    }

    #[test]
    fn replacement_and_rename_preserve_identity_order_and_icon() {
        let initial = one("sample");
        let bytes = testing::effect("sample");
        let off = initial
            .prepare_patch(
                "sample",
                &Patch {
                    name: None,
                    enabled: Some(false),
                },
                &bytes,
                &context(),
            )
            .unwrap()
            .manifest;
        let renamed = off
            .prepare_patch(
                "sample",
                &Patch {
                    name: Some("New Name".into()),
                    enabled: None,
                },
                &bytes,
                &context(),
            )
            .unwrap();
        let (before, icon) = effects::decode(&bytes).unwrap();
        let (mut after, after_icon) =
            effects::decode(&renamed.content.as_ref().unwrap().bytes).unwrap();
        assert_eq!(after.name, "New Name");
        after.name = before.name.clone();
        assert_eq!(before, after);
        assert_eq!(icon, after_icon);
        assert_eq!(renamed.manifest.order, off.order);
        assert!(!renamed.manifest.entries["sample"].enabled);
        assert_ne!(
            renamed.manifest.configuration_etag("sample").unwrap(),
            off.configuration_etag("sample").unwrap()
        );
        let replaced = renamed
            .manifest
            .prepare_put("sample", &bytes, &context())
            .unwrap();
        assert!(!replaced.manifest.entries["sample"].enabled);
        assert_eq!(replaced.manifest.order, off.order);
    }

    #[test]
    fn official_tombstones_prevent_reusing_an_id_for_custom_content() {
        let mut ctx = context();
        let bytes = testing::effect("official");
        ctx.official_hashes
            .insert("official".into(), packaging::sha256(&bytes));
        let official = Manifest::default()
            .prepare_put("official", &bytes, &ctx)
            .unwrap()
            .manifest;
        assert_eq!(official.entries["official"].origin, Origin::Official);
        assert_eq!(
            official
                .prepare_put("official", &bytes, &ctx)
                .unwrap_err()
                .code,
            "official_look_read_only"
        );
        assert_eq!(
            official
                .prepare_patch(
                    "official",
                    &Patch {
                        name: Some("Change".into()),
                        enabled: None
                    },
                    &bytes,
                    &ctx
                )
                .unwrap_err()
                .code,
            "official_look_read_only"
        );
        assert!(official
            .prepare_patch(
                "official",
                &Patch {
                    name: None,
                    enabled: Some(false)
                },
                &bytes,
                &ctx
            )
            .is_ok());
        let removed = official
            .prepare_delete("official", false, &ctx)
            .unwrap()
            .manifest;
        assert!(removed.entries["official"].removed);
        assert_eq!(removed.entries["official"].origin, Origin::Official);
        assert!(removed.configuration("official").is_err());
        let (mut effect, icon) = effects::decode(&bytes).unwrap();
        effect.name = "Forged".into();
        let changed = effect.encode(&icon).unwrap();
        assert_eq!(
            removed
                .prepare_put("official", &changed, &ctx)
                .unwrap_err()
                .code,
            "official_look_read_only"
        );
        assert!(removed.prepare_put("official", &bytes, &context()).is_err());
        assert_eq!(
            removed
                .prepare_put("official", &bytes, &ctx)
                .unwrap()
                .manifest
                .entries["official"]
                .origin,
            Origin::Official
        );
        // A new stable identity is a custom copy.
        effect.id = "copy".into();
        assert_eq!(
            removed
                .prepare_put("copy", &effect.encode(&icon).unwrap(), &ctx)
                .unwrap()
                .manifest
                .entries["copy"]
                .origin,
            Origin::Custom
        );
    }

    #[test]
    fn deletion_requires_known_dependencies_and_explicit_active_policy() {
        let initial = one("sample");
        let mut ctx = context();
        ctx.dependencies_complete = false;
        assert_eq!(
            initial
                .prepare_delete("sample", true, &ctx)
                .unwrap_err()
                .code,
            "dependencies_unavailable"
        );
        ctx.dependencies_complete = true;
        ctx.dependencies
            .insert("sample".into(), vec!["recipe:42".into()]);
        assert_eq!(
            initial
                .prepare_delete("sample", true, &ctx)
                .unwrap_err()
                .code,
            "dependency_in_use"
        );
        ctx.dependencies.clear();
        ctx.selected = Some("sample".into());
        assert_eq!(
            initial
                .prepare_delete("sample", false, &ctx)
                .unwrap_err()
                .code,
            "active_look"
        );
        assert!(
            initial
                .prepare_delete("sample", true, &ctx)
                .unwrap()
                .select_off
        );
        assert!(!initial.entries["sample"].removed);
    }

    #[test]
    fn exact_order_does_not_change_configuration_etags() {
        let initial = one("order")
            .prepare_put("requests", &testing::effect("requests"), &context())
            .unwrap()
            .manifest;
        let reordered = initial
            .prepare_order(&["requests".into(), "order".into()], &context())
            .unwrap()
            .manifest;
        assert_eq!(
            initial.configuration_etag("order").unwrap(),
            reordered.configuration_etag("order").unwrap()
        );
        assert_ne!(initial.order_etag(), reordered.order_etag());
        for ids in [
            vec!["order".into()],
            vec!["order".into(), "order".into()],
            vec!["order".into(), "missing".into()],
        ] {
            assert_eq!(
                initial.prepare_order(&ids, &context()).unwrap_err().code,
                "invalid_order"
            );
        }
    }

    #[test]
    fn invalid_patch_and_unknown_origin_fail_closed() {
        for bytes in [
            br#"{"hidden":true}"#.as_slice(),
            br#"{"name":null}"#,
            br#"{"enabled":1}"#,
            br#"{"enabled":true,"enabled":false}"#,
            b"{}",
        ] {
            assert!(Patch::decode(bytes).is_err());
        }
        assert_eq!(
            Patch::decode(br#"{"enabled":false}"#).unwrap().enabled,
            Some(false)
        );
        let mut unknown = one("sample");
        unknown.entries.get_mut("sample").unwrap().origin = Origin::Unknown;
        assert_eq!(
            unknown
                .prepare_put("sample", &testing::effect("sample"), &context())
                .unwrap_err()
                .code,
            "origin_unknown"
        );
        assert_eq!(
            unknown
                .prepare_patch(
                    "sample",
                    &Patch {
                        name: Some("Changed".into()),
                        enabled: None
                    },
                    &testing::effect("sample"),
                    &context()
                )
                .unwrap_err()
                .code,
            "origin_unknown"
        );
        let mut ctx = context();
        ctx.blocked_ids.insert("sample".into());
        assert_eq!(
            unknown
                .prepare_delete("sample", false, &ctx)
                .unwrap_err()
                .code,
            "ambiguous_look"
        );
    }

    #[test]
    fn reject_reserved_and_colliding_runtime_codes() {
        assert_eq!(effect_code("look20233"), 0x8100);
        assert_eq!(
            Manifest::default()
                .prepare_put("look20233", &testing::effect("look20233"), &context())
                .unwrap_err()
                .code,
            "effect_code_collision"
        );
        let mut ctx = context();
        ctx.reserved_codes.insert(effect_code("sample"));
        assert_eq!(
            Manifest::default()
                .prepare_put("sample", &testing::effect("sample"), &ctx)
                .unwrap_err()
                .code,
            "effect_code_collision"
        );
        let mut seen = BTreeMap::new();
        let (first, second) = (0..=32768)
            .find_map(|i| {
                let id = format!("look-{i}");
                seen.insert(effect_code(&id), id.clone())
                    .map(|first| (first, id))
            })
            .unwrap();
        let initial = one(&first);
        assert_eq!(
            initial
                .prepare_put(&second, &testing::effect(&second), &context())
                .unwrap_err()
                .code,
            "effect_code_collision"
        );
        let removed = initial
            .prepare_delete(&first, false, &context())
            .unwrap()
            .manifest;
        assert_eq!(
            removed
                .prepare_put(&second, &testing::effect(&second), &context())
                .unwrap_err()
                .code,
            "effect_code_collision"
        );
    }

    #[test]
    fn corrupt_content_and_unsupported_camera_do_not_create_candidates() {
        let mut bytes = testing::effect("sample");
        bytes[100] ^= 1;
        assert_eq!(
            Manifest::default()
                .prepare_put("sample", &bytes, &context())
                .unwrap_err()
                .code,
            "invalid_grf"
        );
        let mut ctx = context();
        ctx.monochrome_only = true;
        assert_eq!(
            Manifest::default()
                .prepare_put("sample", &testing::effect("sample"), &ctx)
                .unwrap_err()
                .code,
            "unsupported_look"
        );
        let (mut effect, icon) = effects::decode(&testing::effect("sample")).unwrap();
        effect.fn_defaults = Some([0; 9]);
        let mut ctx = context();
        ctx.supported_features &= !effects::FEATURE_FN_DEFAULTS;
        assert_eq!(
            Manifest::default()
                .prepare_put("sample", &effect.encode(&icon).unwrap(), &ctx)
                .unwrap_err()
                .code,
            "unsupported_look"
        );
        let initial = one("sample");
        assert_eq!(
            initial
                .prepare_patch(
                    "sample",
                    &Patch {
                        name: None,
                        enabled: Some(false)
                    },
                    &bytes,
                    &context()
                )
                .unwrap_err()
                .code,
            "content_unavailable"
        );
    }

    #[test]
    fn enabling_at_loaded_capacity_fails_without_mutating_the_source() {
        let mut initial = one("sample");
        initial.entries.get_mut("sample").unwrap().enabled = false;
        let mut codes = BTreeSet::from([effect_code("sample")]);
        for i in 0..1000 {
            let id = format!("look-{i}");
            if codes.insert(effect_code(&id)) {
                let mut entry = initial.entries["sample"].clone();
                entry.enabled = true;
                initial.entries.insert(id.clone(), entry);
                initial.order.push(id);
                if initial.order.len() == MAX_LOADED + 1 {
                    break;
                }
            }
        }
        assert!(initial.validate().is_ok());
        assert_eq!(
            initial
                .prepare_patch(
                    "sample",
                    &Patch {
                        name: None,
                        enabled: Some(true)
                    },
                    &testing::effect("sample"),
                    &context()
                )
                .unwrap_err()
                .code,
            "look_capacity"
        );
        assert!(!initial.entries["sample"].enabled);
    }

    #[test]
    fn tombstones_and_listed_capacity_have_independent_bounds() {
        let entry = one("sample").entries["sample"].clone();
        let mut manifest = Manifest::default();
        for i in 0..MAX_IDENTITIES {
            let id = format!("look-{i}");
            let mut retained = entry.clone();
            retained.removed = true;
            retained.enabled = false;
            manifest.entries.insert(id, retained);
        }
        assert!(manifest.validate().is_ok());
        manifest.entries.insert("overflow".into(), entry.clone());
        manifest.order.push("overflow".into());
        assert_eq!(manifest.validate().unwrap_err().code, "look_capacity");
        let mut manifest = Manifest::default();
        for i in 0..=MAX_LISTED {
            let id = format!("look-{i}");
            let mut retained = entry.clone();
            retained.enabled = false;
            manifest.entries.insert(id.clone(), retained);
            manifest.order.push(id);
        }
        assert_eq!(manifest.validate().unwrap_err().code, "look_capacity");
    }

    #[test]
    fn verified_catalog_resolves_legacy_origin_without_changing_the_preimage() {
        let bytes = testing::effect("sample");
        let mut legacy = one("sample");
        legacy.entries.get_mut("sample").unwrap().origin = Origin::Unknown;
        let preimage = legacy.encode_binary().unwrap();
        let mut ctx = context();
        ctx.official_hashes
            .insert("sample".into(), packaging::sha256(&bytes));
        assert_eq!(legacy.origin("sample", &ctx).unwrap(), Origin::Official);
        assert_eq!(legacy.encode_binary().unwrap(), preimage);
        assert_eq!(
            legacy.prepare_put("sample", &bytes, &ctx).unwrap_err().code,
            "official_look_read_only"
        );
        assert_eq!(
            legacy
                .prepare_patch(
                    "sample",
                    &Patch {
                        name: Some("Changed".into()),
                        enabled: None,
                    },
                    &bytes,
                    &ctx
                )
                .unwrap_err()
                .code,
            "official_look_read_only"
        );
        let disabled = legacy
            .prepare_patch(
                "sample",
                &Patch {
                    name: None,
                    enabled: Some(false),
                },
                &bytes,
                &ctx,
            )
            .unwrap()
            .manifest;
        assert_eq!(disabled.entries["sample"].origin, Origin::Official);
        let removed = legacy
            .prepare_delete("sample", false, &ctx)
            .unwrap()
            .manifest;
        assert_eq!(removed.entries["sample"].origin, Origin::Official);
        assert!(removed.prepare_put("sample", &bytes, &context()).is_err());
        assert_eq!(
            removed
                .prepare_put("sample", &bytes, &ctx)
                .unwrap()
                .manifest
                .entries["sample"]
                .origin,
            Origin::Official
        );

        ctx.official_hashes.insert("sample".into(), "00".repeat(32));
        assert_eq!(legacy.origin("sample", &ctx).unwrap(), Origin::Unknown);
        assert_eq!(removed.origin("sample", &ctx).unwrap(), Origin::Official);
        assert_eq!(legacy.encode_binary().unwrap(), preimage);
    }

    #[test]
    fn trusted_official_identity_overrides_conflicting_custom_origin() {
        let initial = one("sample");
        let mut ctx = context();
        ctx.official_hashes.insert(
            "sample".into(),
            packaging::sha256(&testing::effect("sample")),
        );
        assert_eq!(
            initial
                .prepare_patch(
                    "sample",
                    &Patch {
                        name: Some("Changed".into()),
                        enabled: None
                    },
                    &testing::effect("sample"),
                    &ctx
                )
                .unwrap_err()
                .code,
            "origin_conflict"
        );
        assert_eq!(
            initial
                .prepare_put("sample", &testing::effect("sample"), &ctx)
                .unwrap_err()
                .code,
            "origin_conflict"
        );
    }

    #[test]
    fn manifest_roundtrip_rejects_invalid_format_duplicates_and_capacity() {
        let initial = one("sample");
        assert_eq!(
            Manifest::decode(&initial.encode().unwrap()).unwrap(),
            initial
        );
        assert!(Manifest::decode(
            br#"{"formatVersion":1,"formatVersion":1,"entries":{},"order":[]}"#
        )
        .is_err());
        let mut future = initial.clone();
        future.format_version += 1;
        assert_eq!(
            future.encode().unwrap_err().code,
            "data_format_incompatible"
        );
        let mut full = Manifest::default();
        for i in 0..=MAX_LOADED {
            let id = format!("look-{i}");
            full.entries
                .insert(id.clone(), initial.entries["sample"].clone());
            full.order.push(id);
        }
        assert_eq!(full.validate().unwrap_err().code, "look_capacity");
        full.entries.get_mut("look-0").unwrap().enabled = false;
        assert!(full.validate().is_ok());
    }
}
