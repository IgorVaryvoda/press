//! Personal recipes: one versioned, strictly parsed model for a conversion
//! recipe, shared by built-in presets, saved personal presets, CLI preset
//! files, and later marketplace and retailer templates.
//!
//! A recipe names its schema, identity, revision, and provenance beside the
//! effective settings, so an output can record the fingerprint of the exact
//! transform that produced it. Parsing is strict on purpose: an unknown schema
//! version, an unknown field, or an out-of-range value refuses before anything
//! is written, imported, or resolved — a recipe that means something else on
//! another version must never silently mean this version's default.

use serde::{Deserialize, Serialize};

use crate::convert::{Format, MaxEdge, Quality};

/// The only schema this Press reads. A higher number refuses with its version
/// named, so a newer recipe never silently becomes an older default.
pub const SCHEMA_VERSION: u32 = 1;

/// The largest recipe file import reads. Recipes are small typed data; a
/// larger file is not a recipe.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

/// Display names stay short enough for the preset row and the report.
pub const MAX_NAME_LEN: usize = 64;

/// Stable identifiers are lowercase slugs: safe in file names on every
/// platform and unambiguous beside display names, which may duplicate.
pub const MAX_ID_LEN: usize = 64;

/// Quality as data: an explicit lossy value or lossless.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeQuality {
    Lossless,
    Lossy(f32),
}

/// The output container. `Keep` is `convert::Format::Same`: each source keeps
/// its own container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeFormat {
    WebP,
    Avif,
    Jpeg,
    Png,
    JpegXl,
    Keep,
}

/// Where a recipe came from. Built-in rows ship with the app; everything a
/// user saves, duplicates, or imports is personal. Retailer-resolved
/// requirements arrive through a different contract, never by editing these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Builtin,
    Personal,
}

/// One saved recipe. Unknown fields refuse: a future schema may add keys with
/// new meaning, and this version must not guess at them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub provenance: Provenance,
    pub format: RecipeFormat,
    pub quality: RecipeQuality,
    /// Capped output edge in pixels, or `None` for the original size.
    pub max_edge: Option<u32>,
    /// libaom speed for AVIF output, or `None` for the built-in default.
    pub avif_speed: Option<u8>,
}

impl Recipe {
    /// The four built-in rows, through the same model personal recipes use, so
    /// the rows and the settings they apply cannot disagree.
    pub fn builtins() -> [Recipe; 4] {
        [
            Recipe {
                schema: SCHEMA_VERSION,
                id: "recommended".into(),
                name: "Recommended".into(),
                revision: 1,
                provenance: Provenance::Builtin,
                format: RecipeFormat::WebP,
                quality: RecipeQuality::Lossy(80.),
                max_edge: None,
                avif_speed: None,
            },
            Recipe {
                schema: SCHEMA_VERSION,
                id: "small-files".into(),
                name: "Small files".into(),
                revision: 1,
                provenance: Provenance::Builtin,
                format: RecipeFormat::Avif,
                quality: RecipeQuality::Lossy(60.),
                max_edge: Some(2400),
                avif_speed: None,
            },
            Recipe {
                schema: SCHEMA_VERSION,
                id: "pixel-perfect".into(),
                name: "Pixel-perfect".into(),
                revision: 1,
                provenance: Provenance::Builtin,
                format: RecipeFormat::WebP,
                quality: RecipeQuality::Lossless,
                max_edge: None,
                avif_speed: None,
            },
            Recipe {
                schema: SCHEMA_VERSION,
                id: "resize-recompress".into(),
                name: "Resize + recompress".into(),
                revision: 1,
                provenance: Provenance::Builtin,
                format: RecipeFormat::Keep,
                quality: RecipeQuality::Lossy(80.),
                max_edge: Some(2400),
                avif_speed: None,
            },
        ]
    }

    /// The effective engine settings: format, quality, max edge, AVIF speed.
    /// Validation at parse time makes the lossy clamp below a no-op.
    pub fn effective(&self) -> (Format, Quality, MaxEdge, Option<u8>) {
        let format = match self.format {
            RecipeFormat::WebP => Format::WebP,
            RecipeFormat::Avif => Format::Avif,
            RecipeFormat::Jpeg => Format::Jpeg,
            RecipeFormat::Png => Format::Png,
            RecipeFormat::JpegXl => Format::JpegXl,
            RecipeFormat::Keep => Format::Same,
        };
        let quality = match self.quality {
            RecipeQuality::Lossless => Quality::LOSSLESS,
            RecipeQuality::Lossy(value) => Quality::lossy(value),
        };
        (format, quality, MaxEdge(self.max_edge), self.avif_speed)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA_VERSION {
            return Err(format!(
                "unsupported recipe schema {} (this Press reads schema {SCHEMA_VERSION})",
                self.schema
            ));
        }
        if self.id.is_empty()
            || self.id.len() > MAX_ID_LEN
            || !self.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
            })
        {
            return Err(format!(
                "recipe id {:?} must be 1-{MAX_ID_LEN} lowercase letters, digits, dashes or underscores",
                self.id
            ));
        }
        if self.name.is_empty()
            || self.name.chars().count() > MAX_NAME_LEN
            || self.name.chars().any(char::is_control)
        {
            return Err("recipe name must be 1-64 printable characters".into());
        }
        if self.revision < 1 {
            return Err("recipe revision starts at 1".into());
        }
        if let RecipeQuality::Lossy(value) = self.quality
            && !(1. ..=100.).contains(&value)
        {
            return Err(format!(
                "recipe quality {value} is outside the supported 1-100"
            ));
        }
        if self.max_edge == Some(0) {
            return Err("recipe max edge must be at least 1 pixel".into());
        }
        if self.avif_speed.is_some_and(|speed| speed > 10) {
            return Err("recipe AVIF speed must be 0-10".into());
        }
        Ok(())
    }

    /// The preset row line, derived from the fields so the row and the settings
    /// it applies cannot disagree. Lossless WebP names its 8-bit scope; every
    /// other limit surfaces at convert time with its reason.
    pub fn summary(&self) -> String {
        let format = match self.format {
            RecipeFormat::WebP => "WebP",
            RecipeFormat::Avif => "AVIF",
            RecipeFormat::Jpeg => "JPEG",
            RecipeFormat::Png => "PNG",
            RecipeFormat::JpegXl => "JXL",
            RecipeFormat::Keep => "Keep format",
        };
        let quality = match self.quality {
            RecipeQuality::Lossless => "lossless".to_string(),
            RecipeQuality::Lossy(value) => format!("quality {}", value.round() as u32),
        };
        let edge = match self.max_edge {
            None => "original size".to_string(),
            Some(edge) => format!("max {edge}px"),
        };
        let mut summary = format!("{format} · {quality} · {edge}");
        if matches!(
            (self.format, self.quality),
            (RecipeFormat::WebP, RecipeQuality::Lossless)
        ) {
            summary.push_str(" · 8-bit sources");
        }
        summary
    }
}

/// Read one recipe file, strictly. Anything the parser or the validator
/// rejects names its reason; nothing half-parsed ever resolves or imports.
pub fn parse_bytes(bytes: &[u8]) -> Result<Recipe, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "recipe files larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let recipe: Recipe =
        serde_json::from_slice(bytes).map_err(|error| format!("recipe does not parse: {error}"))?;
    recipe.validate()?;
    Ok(recipe)
}

/// The fingerprint recorded with each output: the normalized effective
/// settings, not the name or revision. Two recipes that transform identically
/// share it; any output-affecting setting changes it, including AVIF speed.
pub fn fingerprint_settings(
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    avif_speed: Option<u8>,
) -> String {
    use sha2::{Digest, Sha256};
    let speed = avif_speed
        .map(|speed| speed.to_string())
        .unwrap_or_default();
    let canonical = format!(
        "{}|{}|{}|{speed}",
        format.label(),
        quality.label(),
        max_edge.0.map(|edge| edge.to_string()).unwrap_or_default(),
    );
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

/// The most recipes one library holds. Import and save refuse past it: a
/// larger library is a sync problem this app does not have.
pub const MAX_RECIPES: usize = 256;

/// The recipe library beside the settings file. `None` where no config folder
/// resolves; recipe actions stay disabled there instead of failing mid-click.
pub fn dir() -> Option<std::path::PathBuf> {
    crate::settings::path().and_then(|path| path.parent().map(|parent| parent.join("recipes")))
}

/// Every recipe id is already a safe file stem by validation, so joining is
/// joining: no traversal can arrive through an id.
fn file_for(dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    dir.join(format!("{id}.json"))
}

fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_ID_LEN
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(format!("recipe id {id:?} is not a safe file stem"));
    }
    Ok(())
}

/// All saved recipes by name, with the file stems that would not parse.
/// Skips rather than bricks: one externally damaged file must not hide the
/// rest of the library, and import still validates strictly at the door.
pub fn list(dir: &std::path::Path) -> (Vec<Recipe>, Vec<String>) {
    let mut recipes = Vec::new();
    let mut skipped = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (recipes, skipped);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read(&path)
            .ok()
            .and_then(|bytes| parse_bytes(&bytes).ok())
            .filter(|recipe| recipe.id == stem)
        {
            Some(recipe) => recipes.push(recipe),
            None => skipped.push(path.display().to_string()),
        }
    }
    recipes.sort_by(|left, right| left.name.cmp(&right.name));
    (recipes, skipped)
}

/// Write one recipe file atomically: temp file in the same directory, then a
/// rename. Same shape as the Studio result install, without its backup dance.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes).map_err(|error| format!("recipe write failed: {error}"))?;
    #[cfg(windows)]
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("recipe write failed: {error}")
    })
}

fn room_for_one(dir: &std::path::Path) -> Result<(), String> {
    let (recipes, _) = list(dir);
    if recipes.len() >= MAX_RECIPES {
        return Err(format!(
            "the recipe library holds at most {MAX_RECIPES} recipes"
        ));
    }
    Ok(())
}

/// Save a new personal recipe. Refuses an id that is already taken: two rows
/// with one identity would make every later reference ambiguous.
pub fn save(dir: &std::path::Path, recipe: &Recipe) -> Result<(), String> {
    let mut recipe = recipe.clone();
    recipe.provenance = Provenance::Personal;
    let pretty = serde_json::to_string_pretty(&recipe)
        .map_err(|error| format!("recipe does not serialize: {error}"))?;
    parse_bytes(pretty.as_bytes())?;
    check_id(&recipe.id)?;
    std::fs::create_dir_all(dir).map_err(|error| format!("recipe library failed: {error}"))?;
    room_for_one(dir)?;
    let path = file_for(dir, &recipe.id);
    if path.exists() {
        return Err(format!("a recipe named {:?} already exists", recipe.id));
    }
    write_atomic(&path, pretty.as_bytes())
}

/// Rewrite the file for an existing id: renames and duplicates land here
/// after the caller picks the name. Refuses to create: that is `save`.
pub fn overwrite(dir: &std::path::Path, recipe: &Recipe) -> Result<(), String> {
    let mut recipe = recipe.clone();
    recipe.provenance = Provenance::Personal;
    let pretty = serde_json::to_string_pretty(&recipe)
        .map_err(|error| format!("recipe does not serialize: {error}"))?;
    parse_bytes(pretty.as_bytes())?;
    check_id(&recipe.id)?;
    let path = file_for(dir, &recipe.id);
    if !path.exists() {
        return Err(format!("no recipe named {:?} exists", recipe.id));
    }
    write_atomic(&path, pretty.as_bytes())
}

/// Delete one recipe by id. Generated files are elsewhere; deleting a recipe
/// never deletes deliverables.
pub fn remove(dir: &std::path::Path, id: &str) -> Result<(), String> {
    check_id(id)?;
    std::fs::remove_file(file_for(dir, id)).map_err(|_| format!("no recipe named {id:?} exists"))
}

/// Import outside bytes as a personal recipe. The id inside the file wins,
/// but a fork is personal even when its file claims otherwise: only the rows
/// this app ships are built-in, and an id they use is refused.
pub fn import_bytes(dir: &std::path::Path, bytes: &[u8]) -> Result<Recipe, String> {
    let mut recipe = parse_bytes(bytes)?;
    if Recipe::builtins().iter().any(|row| row.id == recipe.id) {
        return Err(format!(
            "recipe id {:?} belongs to a built-in row; rename the import",
            recipe.id
        ));
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("recipe library failed: {error}"))?;
    room_for_one(dir)?;
    if file_for(dir, &recipe.id).exists() {
        return Err(format!("a recipe named {:?} already exists", recipe.id));
    }
    recipe.provenance = Provenance::Personal;
    let pretty = serde_json::to_string_pretty(&recipe)
        .map_err(|error| format!("recipe does not serialize: {error}"))?;
    write_atomic(&file_for(dir, &recipe.id), pretty.as_bytes())?;
    Ok(recipe)
}

/// The exact bytes to hand out on export: what is stored, byte for byte.
pub fn export_bytes(dir: &std::path::Path, id: &str) -> Result<Vec<u8>, String> {
    check_id(id)?;
    std::fs::read(file_for(dir, id)).map_err(|_| format!("no recipe named {id:?} exists"))
}

/// A file stem from a display name: lowercase alphanumerics and dashes,
/// deduplicated against the library. Empty names become `recipe`.
pub fn suggest_id(dir: &std::path::Path, name: &str) -> String {
    let mut stem: String = name
        .to_lowercase()
        .chars()
        .map(|cell| {
            if cell.is_ascii_alphanumeric() {
                cell
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    stem.truncate(MAX_ID_LEN);
    if stem.is_empty() {
        stem = "recipe".into();
    }
    let (recipes, _) = list(dir);
    let taken: std::collections::HashSet<&str> =
        recipes.iter().map(|recipe| recipe.id.as_str()).collect();
    if !taken.contains(stem.as_str()) && !file_for(dir, &stem).exists() {
        return stem;
    }
    for counter in 2.. {
        let candidate = format!("{stem}-{counter}");
        if candidate.len() > MAX_ID_LEN {
            continue;
        }
        if !taken.contains(candidate.as_str()) && !file_for(dir, &candidate).exists() {
            return candidate;
        }
    }
    unreachable!("the counter always finds a free stem")
}
/// Unique scratch libraries for tests that drive the store through other
/// modules. Test-only: production callers resolve `dir()` once per action.
#[cfg(test)]
pub(crate) fn temp_store(name: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "press-recipes-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn personal() -> Recipe {
        Recipe {
            schema: SCHEMA_VERSION,
            id: "my-print".into(),
            name: "My print".into(),
            revision: 3,
            provenance: Provenance::Personal,
            format: RecipeFormat::Jpeg,
            quality: RecipeQuality::Lossy(90.),
            max_edge: Some(2400),
            avif_speed: None,
        }
    }

    #[test]
    fn recipes_round_trip_through_json() {
        for recipe in Recipe::builtins()
            .iter()
            .chain(std::iter::once(&personal()))
        {
            let json = serde_json::to_string_pretty(recipe).unwrap();
            assert_eq!(&parse_bytes(json.as_bytes()).unwrap(), recipe);
        }
    }

    #[test]
    fn the_fingerprint_covers_every_output_affecting_setting() {
        let base = fingerprint_settings(Format::WebP, Quality::lossy(80.), MaxEdge::FULL, None);
        // Pinned: the canonical form must never drift silently, or old outputs
        // stop matching their records.
        assert_eq!(
            base,
            fingerprint_settings(Format::WebP, Quality::lossy(80.), MaxEdge::FULL, None)
        );
        assert_eq!(base.len(), 64, "a hex SHA-256");
        assert_ne!(
            base,
            fingerprint_settings(Format::Avif, Quality::lossy(80.), MaxEdge::FULL, None),
            "the container matters"
        );
        assert_ne!(
            base,
            fingerprint_settings(Format::WebP, Quality::lossy(60.), MaxEdge::FULL, None),
            "the quality matters"
        );
        assert_ne!(
            base,
            fingerprint_settings(Format::WebP, Quality::lossy(80.), MaxEdge(Some(2400)), None),
            "the edge matters"
        );
        assert_ne!(
            base,
            fingerprint_settings(Format::WebP, Quality::lossy(80.), MaxEdge::FULL, Some(9)),
            "the AVIF speed matters"
        );
        assert_ne!(
            base,
            fingerprint_settings(Format::WebP, Quality::LOSSLESS, MaxEdge::FULL, None),
            "lossless differs from any quality number"
        );
        // Names, ids and revisions are provenance, not transform.
        assert_eq!(
            fingerprint_settings(Format::WebP, Quality::lossy(80.), MaxEdge::FULL, None),
            base
        );
    }

    #[test]
    fn strict_parsing_names_its_refusal() {
        let mut future = personal();
        future.schema = SCHEMA_VERSION + 1;
        let refused = parse_bytes(&serde_json::to_vec(&future).unwrap()).unwrap_err();
        assert!(refused.contains("schema 2"), "{refused}");

        let unknown_field = r#"{"schema":1,"id":"x","name":"X","revision":1,"provenance":"personal","format":"webp","quality":"lossless","max_edge":null,"avif_speed":null,"color":"red"}"#;
        let refused = parse_bytes(unknown_field.as_bytes()).unwrap_err();
        assert!(refused.contains("does not parse"), "{refused}");

        // One value per refusal: closures have distinct types, so the cases
        // are built eagerly instead.
        let mut bad_id = personal();
        bad_id.id = "Has Caps!".into();
        let mut bad_name = personal();
        bad_name.name = String::new();
        let mut bad_revision = personal();
        bad_revision.revision = 0;
        let mut bad_low = personal();
        bad_low.quality = RecipeQuality::Lossy(0.);
        let mut bad_high = personal();
        bad_high.quality = RecipeQuality::Lossy(200.);
        let mut bad_edge = personal();
        bad_edge.max_edge = Some(0);
        let mut bad_speed = personal();
        bad_speed.avif_speed = Some(11);
        for (recipe, hint) in [
            (bad_id, "recipe id"),
            (bad_name, "recipe name"),
            (bad_revision, "revision"),
            (bad_low, "quality"),
            (bad_high, "quality"),
            (bad_edge, "max edge"),
            (bad_speed, "AVIF speed"),
        ] {
            let refused = parse_bytes(&serde_json::to_vec(&recipe).unwrap()).unwrap_err();
            assert!(refused.contains(hint), "{hint}: {refused}");
        }
        assert!(
            parse_bytes(&[b'{'; 70_000]).is_err(),
            "oversized files refuse"
        );
        assert!(parse_bytes(b"{\"schema\":1}trailing").is_err());
    }

    #[test]
    fn builtin_rows_resolve_to_the_documented_settings() {
        use crate::convert::{Format, MaxEdge, Quality};
        let settings: Vec<(Format, Quality, MaxEdge, Option<u8>)> =
            Recipe::builtins().iter().map(Recipe::effective).collect();
        assert_eq!(
            settings,
            [
                (Format::WebP, Quality::lossy(80.), MaxEdge::FULL, None),
                (Format::Avif, Quality::lossy(60.), MaxEdge(Some(2400)), None),
                (Format::WebP, Quality::LOSSLESS, MaxEdge::FULL, None),
                (Format::Same, Quality::lossy(80.), MaxEdge(Some(2400)), None),
            ]
        );
    }

    #[test]
    fn effective_settings_match_the_engine_types() {
        let (format, quality, max_edge, speed) = personal().effective();
        assert_eq!(format, Format::Jpeg);
        assert_eq!(quality, Quality::lossy(90.));
        assert_eq!(max_edge, MaxEdge(Some(2400)));
        assert_eq!(speed, None);
        let (format, quality, _, _) = Recipe::builtins()[3].effective();
        assert_eq!((format, quality), (Format::Same, Quality::lossy(80.)));
    }

    fn store(name: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "press-recipes-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn load(dir: &std::path::Path, id: &str) -> Option<Recipe> {
        list(dir).0.into_iter().find(|recipe| recipe.id == id)
    }

    #[test]
    fn save_load_list_remove_round_trip() {
        let dir = store("round-trip");
        let mut beta = personal();
        beta.id = "beta".into();
        beta.name = "Beta".into();
        let mut alpha = personal();
        alpha.id = "alpha".into();
        alpha.name = "Alpha".into();
        save(&dir, &beta).unwrap();
        save(&dir, &alpha).unwrap();
        assert_eq!(load(&dir, "alpha").unwrap(), alpha);
        let (listed, skipped) = list(&dir);
        assert!(skipped.is_empty());
        assert_eq!(
            listed
                .iter()
                .map(|recipe| recipe.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"],
            "the list sorts by name"
        );
        assert!(save(&dir, &alpha).is_err(), "a taken id refuses");
        let mut renamed = alpha;
        renamed.name = "Alpha 2".into();
        overwrite(&dir, &renamed).unwrap();
        assert_eq!(load(&dir, "alpha").unwrap().name, "Alpha 2");
        remove(&dir, "beta").unwrap();
        assert!(load(&dir, "beta").is_none());
        assert!(remove(&dir, "beta").is_err(), "deleting twice fails");
        let (listed, _) = list(&dir);
        assert_eq!(listed.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overwrite_never_creates_and_save_validates() {
        let dir = store("overwrite-guards");
        assert!(overwrite(&dir, &personal()).is_err());
        let mut bad = personal();
        bad.id = "bad".into();
        bad.quality = RecipeQuality::Lossy(0.);
        assert!(save(&dir, &bad).is_err());
        assert!(list(&dir).0.is_empty(), "nothing half-written stays");
        for hostile in ["", "../escape", "a/b", "UPPER", "has space"] {
            assert!(load(&dir, hostile).is_none(), "{hostile:?} never resolves");
            assert!(remove(&dir, hostile).is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn import_is_strict_personal_and_deduplicated() {
        let dir = store("import");
        let mut builtin_claim = personal();
        builtin_claim.id = "imported".into();
        builtin_claim.provenance = Provenance::Builtin;
        let bytes = serde_json::to_vec(&builtin_claim).unwrap();
        let imported = import_bytes(&dir, &bytes).unwrap();
        assert_eq!(imported.provenance, Provenance::Personal);
        assert_eq!(
            load(&dir, "imported").unwrap().provenance,
            Provenance::Personal
        );
        assert!(
            import_bytes(&dir, &bytes).is_err(),
            "the same id twice refuses"
        );
        let mut builtin_id = personal();
        builtin_id.id = "recommended".into();
        assert!(
            import_bytes(&dir, &serde_json::to_vec(&builtin_id).unwrap()).is_err(),
            "built-in ids are protected"
        );
        assert!(import_bytes(&dir, b"{\"schema\":99}").is_err());
        assert!(import_bytes(&dir, &vec![b'x'; 70_000]).is_err());
        let (listed, _) = list(&dir);
        assert_eq!(listed.len(), 1, "refused imports write nothing");
        let exported = export_bytes(&dir, "imported").unwrap();
        assert_eq!(
            import_bytes(&store("import-export"), &exported).unwrap().id,
            "imported",
            "exported bytes import elsewhere"
        );
        assert!(export_bytes(&dir, "missing").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_skips_damage_and_suggests_safe_ids() {
        let dir = store("skips");
        let mut kept = personal();
        kept.id = "kept".into();
        save(&dir, &kept).unwrap();
        std::fs::write(dir.join("broken.json"), b"{\"schema\":").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a recipe").unwrap();
        let (listed, skipped) = list(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            skipped.len(),
            1,
            "the damaged file is named, the text ignored"
        );
        assert_eq!(suggest_id(&dir, "My Print!"), "my-print");
        assert_eq!(suggest_id(&dir, "Kept"), "kept-2", "taken stems dedupe");
        assert_eq!(suggest_id(&dir, "!!!"), "recipe");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
