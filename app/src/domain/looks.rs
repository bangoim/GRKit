//! Film Looks on the camera (`/v1/looks`) and in the official catalog.
//!
//! Kept free of NSH so unit tests run without an engine. Shapes match
//! `grkit-serve` (`look_api.rs`) and the site catalog (`site::CATALOG_SCHEMA`).

use std::collections::BTreeSet;

use serde::Deserialize;

/// The longest Look name the camera menu shows (`effects::NAME_MAX`).
pub const NAME_MAX: usize = 16;
/// The longest effect ID (`effects::ID_MAX`).
pub const ID_MAX: usize = 31;
/// The ID prefix of Looks imported on a phone; no catalog ID starts with it.
pub const IMPORT_PREFIX: &str = "lut-";

/// Where a Look on the card came from, as the camera reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LookSource {
    Official,
    Custom,
    Unknown,
}

/// The film family of a catalog Look, in the order the catalog lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LookGroup {
    Negative,
    Slide,
    Cinema,
    Instant,
    Mono,
}

impl LookGroup {
    pub const ORDER: [LookGroup; 5] = [
        LookGroup::Negative,
        LookGroup::Slide,
        LookGroup::Cinema,
        LookGroup::Instant,
        LookGroup::Mono,
    ];

    /// String key of the group's label (`strings.rs`).
    pub fn label_key(self) -> &'static str {
        match self {
            LookGroup::Negative => "looks.group.negative",
            LookGroup::Slide => "looks.group.slide",
            LookGroup::Cinema => "looks.group.cinema",
            LookGroup::Instant => "looks.group.instant",
            LookGroup::Mono => "looks.group.mono",
        }
    }

    pub fn monochrome(self) -> bool {
        self == LookGroup::Mono
    }
}

/// The catalog filter of the All Looks list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookFilter {
    All,
    Color,
    Mono,
}

impl LookFilter {
    pub fn accepts(self, group: LookGroup) -> bool {
        match self {
            LookFilter::All => true,
            LookFilter::Color => !group.monochrome(),
            LookFilter::Mono => group.monochrome(),
        }
    }
}

/// Catalog groups of the official Looks (the site's `effectGroups.ts`).
const GROUPS: &[(&str, LookGroup)] = &[
    ("aerocolor-iv-v1", LookGroup::Negative),
    ("c200-premier-v1", LookGroup::Negative),
    ("ektar-100-edge-v1", LookGroup::Negative),
    ("fuji-160c-v1", LookGroup::Negative),
    ("gold-200-premier-v1", LookGroup::Negative),
    ("portra-160-vc-v1", LookGroup::Negative),
    ("portra-400-endura-v1", LookGroup::Negative),
    ("portra-800-push2-v1", LookGroup::Negative),
    ("pro-400h-crystal-v1", LookGroup::Negative),
    ("ultramax-400-ultra-v1", LookGroup::Negative),
    ("vista-200-v1", LookGroup::Negative),
    ("xtra-400-crystal-v1", LookGroup::Negative),
    ("astia-100f-v1", LookGroup::Slide),
    ("ektachrome-e100-v1", LookGroup::Slide),
    ("kodachrome-64-v1", LookGroup::Slide),
    ("precisa-100-v1", LookGroup::Slide),
    ("provia-100f-v1", LookGroup::Slide),
    ("velvia-100-v1", LookGroup::Slide),
    ("eterna-500t-3523xd-v1", LookGroup::Cinema),
    ("vision3-250d-2383-v1", LookGroup::Cinema),
    ("vision3-500t-supra-v1", LookGroup::Cinema),
    ("instax-v1", LookGroup::Instant),
    ("delta-100-v1", LookGroup::Mono),
    ("double-x-5222-mono-v1", LookGroup::Mono),
    ("hp5-plus-400-mono-v1", LookGroup::Mono),
    ("neopan-acros-100-v1", LookGroup::Mono),
    ("red-filter-mono-v1", LookGroup::Mono),
    ("t-max-400-v1", LookGroup::Mono),
    ("trix-400-mono-v1", LookGroup::Mono),
    ("xp2-super-v1", LookGroup::Mono),
];

/// The group of a Look. An unknown ID falls back to color negative or B&W.
pub fn group_of(id: &str, monochrome: bool) -> LookGroup {
    GROUPS
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, group)| *group)
        .unwrap_or(if monochrome {
            LookGroup::Mono
        } else {
            LookGroup::Negative
        })
}

/// One Look on the card, with its runtime state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub id: String,
    pub name: String,
    pub content_hash: String,
    /// Shown in the camera menu.
    pub enabled: bool,
    /// Position in the camera menu, from 0.
    pub position: usize,
    pub source: LookSource,
    /// Loaded by the runtime (at most `max_loaded` are).
    pub loaded: bool,
    /// The Look in use.
    pub selected: bool,
    /// Recipes and dial sets that use the Look.
    pub dependencies: Vec<String>,
    pub dependencies_complete: bool,
    /// The camera blocks changes, for example for duplicate files.
    pub blocked: bool,
}

impl Look {
    /// Only phone-made Looks change their name, cover and content.
    pub fn editable(&self) -> bool {
        self.source == LookSource::Custom
    }
}

/// The camera's Looks, in camera menu order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LooksSnapshot {
    pub looks: Vec<Look>,
    pub loaded: usize,
    pub max_loaded: usize,
    pub max_listed: usize,
    pub boot_id: String,
    pub card_id: String,
}

impl LooksSnapshot {
    pub fn get(&self, id: &str) -> Option<&Look> {
        self.looks.iter().find(|look| look.id == id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.looks.iter().map(|look| look.id.clone()).collect()
    }

    /// Room for one more enabled Look.
    pub fn has_room(&self) -> bool {
        self.looks.len() < self.max_listed && self.loaded < self.max_loaded
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireList {
    looks: Vec<WireLook>,
    counts: WireCounts,
    limits: WireLimits,
    boot_id: String,
    card_id: String,
}

#[derive(Deserialize)]
struct WireCounts {
    loaded: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLimits {
    listed_looks: usize,
    loaded_looks: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLook {
    id: String,
    name: String,
    content_hash: String,
    enabled: bool,
    position: usize,
    runtime: WireRuntime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRuntime {
    source: LookSource,
    loaded: bool,
    selected: bool,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    dependencies_complete: bool,
    #[serde(default)]
    blocked: bool,
}

/// Parse a `GET /v1/looks` body.
pub fn parse_looks(body: &[u8]) -> Result<LooksSnapshot, String> {
    let wire: WireList = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let mut looks: Vec<Look> = wire
        .looks
        .into_iter()
        .map(|look| Look {
            id: look.id,
            name: look.name,
            content_hash: look.content_hash,
            enabled: look.enabled,
            position: look.position,
            source: look.runtime.source,
            loaded: look.runtime.loaded,
            selected: look.runtime.selected,
            dependencies: look.runtime.dependencies,
            dependencies_complete: look.runtime.dependencies_complete,
            blocked: look.runtime.blocked,
        })
        .collect();
    looks.sort_by_key(|look| look.position);
    Ok(LooksSnapshot {
        looks,
        loaded: wire.counts.loaded,
        max_loaded: wire.limits.loaded_looks,
        max_listed: wire.limits.listed_looks,
        boot_id: wire.boot_id,
        card_id: wire.card_id,
    })
}

/// Why a Looks request failed, in terms the UI explains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookError {
    /// The phone lost the camera network.
    ConnectionLost,
    /// The card or runtime is not ready yet.
    NotReady,
    /// Recipes or dial sets use the Look.
    InUse,
    /// Deleting the Look in use needs the camera to switch to Off.
    ActiveLook,
    /// Official Looks keep their name, cover and content.
    OfficialReadOnly,
    /// The camera cannot load or list another Look.
    NoRoom,
    /// A software update waits on the camera.
    UpdatePending,
    /// Phone changes are off on the camera.
    ChangesOff,
    /// Another change is running.
    Busy,
    /// The Look changed since the phone read it.
    Changed,
    /// This camera does not apply Look changes while it runs.
    LiveChangesUnavailable,
    InvalidName,
    NotFound,
    /// The file is not a LUT or preset the phone can convert.
    ImportFailed(String),
    Service(String),
}

impl LookError {
    /// String keys of the title and body (`strings.rs`).
    pub fn keys(&self) -> (&'static str, &'static str) {
        match self {
            LookError::ConnectionLost => ("looks.error.lost_title", "looks.error.lost_body"),
            LookError::NotReady => ("looks.error.not_ready_title", "looks.error.not_ready_body"),
            LookError::InUse => ("looks.error.in_use_title", "looks.error.in_use_body"),
            LookError::ActiveLook => ("looks.error.active_title", "looks.error.active_body"),
            LookError::OfficialReadOnly => {
                ("looks.error.official_title", "looks.error.official_body")
            }
            LookError::NoRoom => ("looks.error.full_title", "looks.error.full_body"),
            LookError::UpdatePending => ("looks.error.pending_title", "looks.error.pending_body"),
            LookError::ChangesOff => ("looks.error.off_title", "looks.error.off_body"),
            LookError::Busy => ("looks.error.busy_title", "looks.error.busy_body"),
            LookError::Changed => ("looks.error.changed_title", "looks.error.changed_body"),
            LookError::LiveChangesUnavailable => {
                ("looks.error.live_title", "looks.error.live_body")
            }
            LookError::InvalidName => ("looks.error.name_title", "looks.error.name_body"),
            LookError::NotFound => ("looks.error.gone_title", "looks.error.gone_body"),
            LookError::ImportFailed(_) => ("looks.error.import_title", "looks.error.import_body"),
            LookError::Service(_) => ("looks.error.service_title", "looks.error.service_body"),
        }
    }
}

/// Map an error answer: a problem document (`code`) or a failed request
/// record (`error.code`).
pub fn look_error(status: u16, body: &str) -> LookError {
    let value = serde_json::from_str::<serde_json::Value>(body).unwrap_or_default();
    let problem = if value.get("code").is_some() {
        &value
    } else {
        value.get("error").unwrap_or(&value)
    };
    let code = problem.get("code").and_then(|v| v.as_str()).unwrap_or("");
    match code {
        "dependency_in_use" | "dependencies_unavailable" => return LookError::InUse,
        "active_look" => return LookError::ActiveLook,
        "official_look_read_only" | "origin_unknown" | "origin_conflict" => {
            return LookError::OfficialReadOnly;
        }
        "look_capacity" => return LookError::NoRoom,
        "software_pending" => return LookError::UpdatePending,
        "writes_disabled" => return LookError::ChangesOff,
        "camera_busy" | "commit_in_progress" | "request_id_conflict" | "runtime_busy" => {
            return LookError::Busy;
        }
        "hot_reload_unavailable" => return LookError::LiveChangesUnavailable,
        "invalid_name" => return LookError::InvalidName,
        "look_not_found" => return LookError::NotFound,
        "card_unavailable" | "runtime_unavailable" => return LookError::NotReady,
        _ => {}
    }
    match status {
        412 => LookError::Changed,
        404 => LookError::NotFound,
        503 => LookError::NotReady,
        _ => LookError::Service(
            problem
                .get("detail")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Camera returned HTTP {status}.")),
        ),
    }
}

/// A Look name the camera accepts: 1 to 16 printable ASCII characters
/// without spaces at either end.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name == name.trim()
        && name.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Words of LUT file names that name an encoding or a format, not the look.
fn technical(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    matches!(
        w,
        "flog"
            | "f-log"
            | "flog2"
            | "f-log2"
            | "flog2c"
            | "vlog"
            | "v-log"
            | "slog"
            | "slog2"
            | "slog3"
            | "s-log3"
            | "logc"
            | "logc3"
            | "logc4"
            | "log"
            | "to"
            | "bt709"
            | "bt.709"
            | "rec709"
            | "rec.709"
            | "709"
            | "srgb"
            | "p3"
            | "displayp3"
            | "d65"
            | "lut"
            | "luts"
            | "cube"
            | "xmp"
    ) || (w.ends_with("grid") && w[..w.len() - 4].bytes().all(|b| b.is_ascii_digit()))
}

/// A Look name from a file name: the stem without technical words, cut to the
/// whole words that fit 16 characters.
pub fn name_from_file(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let words: Vec<String> = stem
        .split(|c: char| c == '_' || c.is_whitespace() || !(c.is_ascii_graphic()))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    let kept: Vec<&String> = words.iter().filter(|w| !technical(w)).collect();
    let chosen: Vec<&String> = if kept.is_empty() {
        words.iter().collect()
    } else {
        kept
    };
    let mut text = String::new();
    for word in chosen {
        let next = if text.is_empty() {
            word.clone()
        } else {
            format!("{text} {word}")
        };
        if next.len() > NAME_MAX {
            if text.is_empty() {
                text = word.chars().take(NAME_MAX).collect();
            }
            break;
        }
        text = next;
    }
    let text = text.trim().to_owned();
    if valid_name(&text) {
        text
    } else {
        "Imported Look".into()
    }
}

/// A new Look ID for `name`: `lut-` and the lowercase letters and digits of
/// the name, with a number when the ID is taken.
pub fn import_id(name: &str, taken: &BTreeSet<String>) -> String {
    let mut slug = String::new();
    for c in name.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "look" } else { slug };
    let mut base: String = format!("{IMPORT_PREFIX}{slug}")
        .chars()
        .take(ID_MAX - 3)
        .collect();
    while base.ends_with('-') {
        base.pop();
    }
    let mut id = base.clone();
    let mut number = 2;
    while taken.contains(&id) {
        id = format!("{base}-{number}");
        number += 1;
    }
    id
}

/// Film brands as the catalog covers write them.
const BRANDS: &[(&str, &str)] = &[
    ("kodak", "Kodak"),
    ("fuji", "FUJIFILM"),
    ("fujifilm", "FUJIFILM"),
    ("ilford", "ILFORD"),
    ("agfa", "AGFA"),
    ("polaroid", "Polaroid"),
    ("rollei", "Rollei"),
    ("cinestill", "CineStill"),
    ("lomography", "Lomography"),
];

const COVER_TEXT_MAX: usize = 12;

fn cover_text(text: &str) -> String {
    text.chars()
        .take(COVER_TEXT_MAX)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The `icon` of an imported Look: a film box like the catalog covers. A
/// known brand becomes the brand; the first film speed (400, 200T) the number.
pub fn cover_for_name(name: &str) -> serde_json::Value {
    let words: Vec<&str> = name.split_whitespace().collect();
    let brand_at = words
        .iter()
        .position(|w| BRANDS.iter().any(|(k, _)| w.eq_ignore_ascii_case(k)));
    let brand = brand_at
        .and_then(|i| {
            BRANDS
                .iter()
                .find(|(k, _)| words[i].eq_ignore_ascii_case(k))
                .map(|(_, v)| (*v).to_owned())
        })
        .unwrap_or_else(|| "GRKit".into());
    let is_number = |w: &str| {
        let digits = w.bytes().take_while(u8::is_ascii_digit).count();
        digits >= 2
            && w.len() - digits <= 1
            && w.bytes().skip(digits).all(|b| b.is_ascii_alphabetic())
    };
    let number = words.iter().find(|w| is_number(w)).map(|w| cover_text(w));
    let rest: Vec<&str> = words
        .iter()
        .enumerate()
        .filter(|(i, w)| Some(*i) != brand_at && Some(cover_text(w)) != number)
        .map(|(_, w)| *w)
        .collect();
    let mut title = cover_text(&rest.join(" ").to_uppercase());
    if title.is_empty() {
        title = cover_text(&name.to_uppercase());
    }
    if title.is_empty() {
        title = "LOOK".into();
    }
    let mut icon = serde_json::json!({
        "template": "box",
        "brand": brand,
        "name": title,
        "band": true,
    });
    if let Some(number) = number.filter(|n| !n.is_empty()) {
        icon["number"] = serde_json::json!(number);
    }
    icon
}

/// Move `ids[from]` to `to`, keeping the others in order.
pub fn move_id(ids: &mut Vec<String>, from: usize, to: usize) {
    if from >= ids.len() || to >= ids.len() || from == to {
        return;
    }
    let id = ids.remove(from);
    ids.insert(to, id);
}

/// One official Look of the site catalog for a firmware target.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogLook {
    pub id: String,
    pub name: String,
    pub monochrome: bool,
    /// File name under `effects/<target>/`.
    pub file: String,
    pub sha256: String,
    /// The film box drawn on the site, when the Look has one.
    #[serde(default)]
    pub cover: Option<CatalogCover>,
}

/// The film box fields of a catalog Look.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogCover {
    #[serde(default)]
    pub brand: Option<String>,
}

/// The site catalog of one firmware target.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Catalog {
    pub target: String,
    pub base_gamma: Vec<u16>,
    #[serde(rename = "effects")]
    pub looks: Vec<CatalogLook>,
}

/// Parse `effects/<target>/catalog.json`.
pub fn parse_catalog(bytes: &[u8]) -> Result<Catalog, String> {
    #[derive(Deserialize)]
    struct Schema {
        schema: String,
    }
    let schema: Schema = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if schema.schema != grkit_core::site::CATALOG_SCHEMA {
        return Err(format!("Unsupported catalog schema: {}", schema.schema));
    }
    let catalog: Catalog = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if catalog.base_gamma.len() != 256 {
        return Err("Catalog base gamma must have 256 values".into());
    }
    for look in &catalog.looks {
        let valid_file = look.file.len() == 12
            && look.file.ends_with(".GRF")
            && look.file[..8].bytes().all(|b| b.is_ascii_hexdigit());
        if !grkit_core::effects::valid_id(&look.id) || !valid_file {
            return Err(format!("Invalid catalog entry: {}", look.id));
        }
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"revision":"sha256:r","generation":3,"looks":[
        {"id":"trix-400-mono-v1","name":"Tri-X 400","contentHash":"b","enabled":true,"position":1,
         "runtime":{"id":"trix-400-mono-v1","source":"official","loaded":true,"selected":false,"dependencies":[],"dependenciesComplete":true,"blocked":false}},
        {"id":"portra-400-endura-v1","name":"Portra 400","contentHash":"a","enabled":true,"position":0,
         "runtime":{"id":"portra-400-endura-v1","source":"official","loaded":true,"selected":true,"dependencies":["recipe:1"],"dependenciesComplete":true,"blocked":false}}],
        "counts":{"listed":2,"loaded":2},"limits":{"listedLooks":64,"loadedLooks":40},
        "bootId":"boot","cardId":"card","snapshotSeq":4}"#;

    #[test]
    fn parses_the_service_list_in_menu_order() {
        let snapshot = parse_looks(LIST.as_bytes()).unwrap();
        assert_eq!(snapshot.ids(), ["portra-400-endura-v1", "trix-400-mono-v1"]);
        let portra = snapshot.get("portra-400-endura-v1").unwrap();
        assert!(portra.selected);
        assert_eq!(portra.dependencies, ["recipe:1"]);
        assert_eq!(portra.source, LookSource::Official);
        assert_eq!((snapshot.loaded, snapshot.max_loaded), (2, 40));
        assert_eq!(snapshot.boot_id, "boot");
        assert!(snapshot.has_room());
    }

    #[test]
    fn maps_problem_documents_and_failed_records() {
        assert_eq!(
            look_error(409, r#"{"status":409,"code":"dependency_in_use"}"#),
            LookError::InUse
        );
        assert_eq!(
            look_error(
                409,
                r#"{"requestId":"x","status":"failed","error":{"status":409,"code":"active_look"}}"#
            ),
            LookError::ActiveLook
        );
        assert_eq!(look_error(412, "{}"), LookError::Changed);
        assert_eq!(
            look_error(409, r#"{"code":"software_pending"}"#),
            LookError::UpdatePending
        );
        assert_eq!(
            look_error(422, r#"{"code":"x","detail":"Bad"}"#),
            LookError::Service("Bad".into())
        );
    }

    #[test]
    fn groups_fall_back_by_monochrome() {
        assert_eq!(group_of("velvia-100-v1", false), LookGroup::Slide);
        assert_eq!(group_of("lut-x", true), LookGroup::Mono);
        assert_eq!(group_of("lut-x", false), LookGroup::Negative);
        assert!(LookFilter::Color.accepts(LookGroup::Cinema));
        assert!(!LookFilter::Color.accepts(LookGroup::Mono));
    }

    #[test]
    fn names_follow_the_camera_rule() {
        assert!(valid_name("Portra 400"));
        assert!(!valid_name(""));
        assert!(!valid_name(" Portra"));
        assert!(!valid_name("A name that is too long"));
        assert!(!valid_name("Fuji\u{e9}"));
        assert_eq!(
            name_from_file("Kodak_Portra_400_F-Log2_to_Rec709.cube"),
            "Kodak Portra 400"
        );
        assert_eq!(name_from_file("cube.cube"), "cube");
        assert_eq!(
            name_from_file("Superlongfilenamewithoutspaces.cube"),
            "Superlongfilenam"
        );
    }

    #[test]
    fn import_ids_are_unique_and_valid() {
        let mut taken = BTreeSet::new();
        let id = import_id("Kodak Portra 400", &taken);
        assert_eq!(id, "lut-kodak-portra-400");
        taken.insert(id);
        assert_eq!(
            import_id("Kodak Portra 400", &taken),
            "lut-kodak-portra-400-2"
        );
        let long = import_id("A very long name for a look indeed", &BTreeSet::new());
        assert!(long.len() <= ID_MAX - 3);
        assert!(grkit_core::effects::valid_id(&long));
    }

    #[test]
    fn covers_render_on_the_camera() {
        for name in [
            "Kodak Portra 400",
            "My LUT",
            "x",
            "Cinestill 800T",
            "ABCDEFGHIJKLMNOP",
        ] {
            let icon = cover_for_name(name);
            grkit_core::effects::icon_pixels(&icon)
                .unwrap_or_else(|e| panic!("{name}: {e} ({icon})"));
        }
        let icon = cover_for_name("Kodak Portra 400");
        assert_eq!(icon["brand"], "Kodak");
        assert_eq!(icon["name"], "PORTRA");
        assert_eq!(icon["number"], "400");
    }

    #[test]
    fn moves_ids() {
        let mut ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        move_id(&mut ids, 0, 2);
        assert_eq!(ids, ["b", "c", "a"]);
        move_id(&mut ids, 2, 0);
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn parses_the_site_catalog() {
        let body = serde_json::json!({
            "schema": grkit_core::site::CATALOG_SCHEMA,
            "target": "gr3-2.21",
            "base_gamma": vec![0u16; 256],
            "preview": {},
            "effects": [{"id":"velvia-100-v1","name":"Velvia 100","monochrome":false,
                "file":"0A1B2C3D.GRF","sha256":"00","icon":"","look":"x","origin":"x","license":"x","preview":"p"}]
        });
        let catalog = parse_catalog(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(catalog.looks[0].name, "Velvia 100");
        let mut bad = body.clone();
        bad["effects"][0]["file"] = "../x".into();
        assert!(parse_catalog(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}
