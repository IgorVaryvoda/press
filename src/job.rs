//! Local product-set jobs: which files belong to which product views, and
//! what was decided about them. Optional grouping over the folder workflow:
//! quick conversion never needs a job, and a job never phones anywhere.
//!
//! Files are versioned JSON beside the settings file, written atomically and
//! parsed strictly like recipes. A portable export carries the same job with
//! paths relative to one base plus that base as a display hint; importing
//! chooses a new root, and anything that does not resolve there becomes a
//! relink state instead of a guess. The model holds no credentials, tokens,
//! prompts, or contact details, so there is nothing private to strip.

use serde::{Deserialize, Serialize};

/// The only schema this Press reads. Newer refuses named, like recipes.
pub const SCHEMA_VERSION: u32 = 1;

/// Jobs hold mappings, so the cap sits above the recipe file cap.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// More than this many saved jobs is a catalog problem this app does not have.
pub const MAX_JOBS: usize = 64;

/// Stable identifiers share the recipe slug rules, so one validator covers
/// both libraries.
pub use crate::recipe::MAX_ID_LEN;
pub use crate::recipe::MAX_NAME_LEN;

/// A file as observed when it was mapped: absolute locally, with the size and
/// mtime the mapping was made at. Later bytes or a later tick invalidate the
/// mapping visibly instead of moving it silently.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub path: std::path::PathBuf,
    pub bytes: u64,
    /// Seconds since the Unix epoch; `None` where the filesystem would not say.
    pub modified: Option<u64>,
}

/// One named view of a product: main image, detail, lifestyle, or a custom
/// role the job needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub id: String,
    pub label: String,
    pub required: bool,
}

/// One file mapped to one role. A role repeats across rows when it takes
/// several files; an id identifies the row for stable updates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub id: String,
    pub role_id: String,
    pub source: SourceRef,
}

/// Canonical server identity, reserved for the supplier integration. Local
/// jobs never fill it, never read it, and never grant anything from it: a
/// filename that resembles a SKU is a hint, not an assignment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerBinding {
    pub workspace: String,
    pub supplier: String,
    pub product: String,
    pub slot: String,
}

/// One product's required views, mapped files, and optional server binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductSet {
    pub id: String,
    pub name: String,
    /// A display and matching hint, never a global identifier: equal SKUs for
    /// different recipients stay distinct products.
    pub sku_hint: String,
    pub roles: Vec<Role>,
    pub mappings: Vec<Mapping>,
    pub binding: Option<ServerBinding>,
}

/// A local job: source roots, an optional target recipe reference, and its
/// product sets. Unknown fields refuse: a future schema may add keys with new
/// meaning, and this version must not guess at them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub source_roots: Vec<std::path::PathBuf>,
    /// Personal recipe id resolved at prepare time. A missing recipe refuses
    /// with its name rather than falling back to whatever is selected.
    pub target_recipe: Option<String>,
    pub products: Vec<ProductSet>,
}

/// The portable form: identical except every path is relative to one base,
/// with that base kept as a display hint only. Importing never reads through
/// the hint; it joins the new root the user chose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableJob {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub base_hint: String,
    pub source_roots: Vec<std::path::PathBuf>,
    pub target_recipe: Option<String>,
    pub products: Vec<PortableProduct>,
}

/// A product set with paths relative to the export base.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableProduct {
    pub id: String,
    pub name: String,
    pub sku_hint: String,
    pub roles: Vec<Role>,
    pub mappings: Vec<PortableMapping>,
    pub binding: Option<ServerBinding>,
}

/// One mapping with its source relative to the export base.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableMapping {
    pub id: String,
    pub role_id: String,
    pub source: PortableSource,
}

/// A source reference relative to the export base.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSource {
    pub path: std::path::PathBuf,
    pub bytes: u64,
    pub modified: Option<u64>,
}

impl Job {
    /// A fresh job over absolute source roots. Roots must exist; anything
    /// else is a relink problem at open, not a job.
    pub fn new(
        id: String,
        name: String,
        source_roots: Vec<std::path::PathBuf>,
    ) -> Result<Self, String> {
        let job = Self {
            schema: SCHEMA_VERSION,
            id,
            name,
            revision: 1,
            source_roots,
            target_recipe: None,
            products: Vec::new(),
        };
        job.validate()?;
        for root in &job.source_roots {
            if !root.is_absolute() {
                return Err(format!("job root {} is not absolute", root.display()));
            }
        }
        Ok(job)
    }

    /// Next revision after a mutation. Callers bump; the store never invents.
    pub fn bumped(&self) -> Self {
        let mut next = self.clone();
        next.revision = next.revision.saturating_add(1);
        next
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA_VERSION {
            return Err(format!(
                "unsupported job schema {} (this Press reads schema {SCHEMA_VERSION})",
                self.schema
            ));
        }
        check_slug(&self.id, "job")?;
        check_name(&self.name, "job")?;
        if self.revision < 1 {
            return Err("job revision starts at 1".into());
        }
        if self.source_roots.is_empty() {
            return Err("a job needs at least one source root".into());
        }
        let mut products = std::collections::HashSet::new();
        for product in &self.products {
            check_slug(&product.id, "product")?;
            check_name(&product.name, "product")?;
            if !products.insert(product.id.as_str()) {
                return Err(format!("duplicate product id {:?}", product.id));
            }
            let mut roles = std::collections::HashSet::new();
            for role in &product.roles {
                check_slug(&role.id, "role")?;
                if role.label.is_empty() || role.label.chars().count() > MAX_NAME_LEN {
                    return Err("role labels must be 1-64 printable characters".into());
                }
                if !roles.insert(role.id.as_str()) {
                    return Err(format!("duplicate role id {:?}", role.id));
                }
            }
            let mut mappings = std::collections::HashSet::new();
            for mapping in &product.mappings {
                check_slug(&mapping.id, "mapping")?;
                if !mappings.insert(mapping.id.as_str()) {
                    return Err(format!("duplicate mapping id {:?}", mapping.id));
                }
                if !roles.contains(mapping.role_id.as_str()) {
                    return Err(format!(
                        "mapping {:?} names unknown role {:?}",
                        mapping.id, mapping.role_id
                    ));
                }
                if mapping.source.path.as_os_str().is_empty() {
                    return Err(format!("mapping {:?} has no source path", mapping.id));
                }
            }
        }
        Ok(())
    }

    /// Portable form with every path relative to `base`. Any path outside the
    /// base refuses by name: an export that silently dropped files would
    /// deliver a smaller job than the one reviewed.
    pub fn to_portable(&self, base: &std::path::Path) -> Result<PortableJob, String> {
        fn relative(
            base: &std::path::Path,
            path: &std::path::Path,
        ) -> Result<std::path::PathBuf, String> {
            path.strip_prefix(base)
                .map(PathBuf::from)
                .map_err(|_| format!("{} is outside the export base", path.display()))
        }
        use std::path::PathBuf;
        Ok(PortableJob {
            schema: self.schema,
            id: self.id.clone(),
            name: self.name.clone(),
            revision: self.revision,
            base_hint: base.display().to_string(),
            source_roots: self
                .source_roots
                .iter()
                .map(|root| relative(base, root))
                .collect::<Result<_, _>>()?,
            target_recipe: self.target_recipe.clone(),
            products: self
                .products
                .iter()
                .map(|product| {
                    Ok(PortableProduct {
                        id: product.id.clone(),
                        name: product.name.clone(),
                        sku_hint: product.sku_hint.clone(),
                        roles: product.roles.clone(),
                        mappings: product
                            .mappings
                            .iter()
                            .map(|mapping| {
                                Ok(PortableMapping {
                                    id: mapping.id.clone(),
                                    role_id: mapping.role_id.clone(),
                                    source: PortableSource {
                                        path: relative(base, &mapping.source.path)?,
                                        bytes: mapping.source.bytes,
                                        modified: mapping.source.modified,
                                    },
                                })
                            })
                            .collect::<Result<_, String>>()?,
                        binding: product.binding.clone(),
                    })
                })
                .collect::<Result<_, String>>()?,
        })
    }

    /// Rebase a portable job under the root its new owner chose. Files that
    /// do not resolve there are not guessed at: they arrive as relink states
    /// for the mapping engine to surface.
    pub fn from_portable(portable: &PortableJob, root: &std::path::Path) -> Result<Self, String> {
        use std::path::PathBuf;
        if portable.schema != SCHEMA_VERSION {
            return Err(format!(
                "unsupported job schema {} (this Press reads schema {SCHEMA_VERSION})",
                portable.schema
            ));
        }
        if !root.is_absolute() {
            return Err(format!("job root {} is not absolute", root.display()));
        }
        let join = |relative: &std::path::Path| {
            use std::path::Component;
            // `has_root` catches what `is_absolute` misses: on Windows a path
            // like `/abs.png` has a root but no prefix, and joining it would
            // rebase the mapping onto the drive root instead of the root its
            // new owner chose. A parent component escapes the root on every
            // platform, including from a nested `sub/../../escape`. Portable
            // paths are never either; refuse them.
            if relative.is_absolute()
                || relative.has_root()
                || relative
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
            {
                return Err(format!(
                    "portable path {} is not relative",
                    relative.display()
                ));
            }
            Ok(root.join(relative))
        };
        let job = Self {
            schema: portable.schema,
            id: portable.id.clone(),
            name: portable.name.clone(),
            revision: portable.revision,
            source_roots: portable
                .source_roots
                .iter()
                .map(|path| join(path))
                .collect::<Result<Vec<PathBuf>, String>>()?,
            target_recipe: portable.target_recipe.clone(),
            products: portable
                .products
                .iter()
                .map(|product| {
                    Ok(ProductSet {
                        id: product.id.clone(),
                        name: product.name.clone(),
                        sku_hint: product.sku_hint.clone(),
                        roles: product.roles.clone(),
                        mappings: product
                            .mappings
                            .iter()
                            .map(|mapping| {
                                Ok(Mapping {
                                    id: mapping.id.clone(),
                                    role_id: mapping.role_id.clone(),
                                    source: SourceRef {
                                        path: join(&mapping.source.path)?,
                                        bytes: mapping.source.bytes,
                                        modified: mapping.source.modified,
                                    },
                                })
                            })
                            .collect::<Result<_, String>>()?,
                        binding: product.binding.clone(),
                    })
                })
                .collect::<Result<_, String>>()?,
        };
        job.validate()?;
        Ok(job)
    }
}

fn check_slug(id: &str, what: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_ID_LEN
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(format!(
            "{what} id {id:?} must be 1-{MAX_ID_LEN} lowercase letters, digits, dashes or underscores"
        ));
    }
    Ok(())
}

fn check_name(name: &str, what: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control)
    {
        return Err(format!(
            "{what} name must be 1-{MAX_NAME_LEN} printable characters"
        ));
    }
    Ok(())
}

/// Read one job file, strictly. Mirrors the recipe door: size cap, schema
/// pin, full validation, and nothing half-parsed.
pub fn parse_bytes(bytes: &[u8]) -> Result<Job, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "job files larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let job: Job =
        serde_json::from_slice(bytes).map_err(|error| format!("job does not parse: {error}"))?;
    job.validate()?;
    Ok(job)
}

/// The job library beside the settings file, mirroring the recipe library.
pub fn dir() -> Option<std::path::PathBuf> {
    crate::settings::path().and_then(|path| path.parent().map(|parent| parent.join("jobs")))
}

/// Whether an id already has a file. Imports refuse collisions instead of
/// silently replacing a different job that shares an id.
pub fn exists(dir: &std::path::Path, id: &str) -> bool {
    check_slug(id, "job").is_ok() && file_for(dir, id).exists()
}
fn file_for(dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    dir.join(format!("{id}.json"))
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes).map_err(|error| format!("job write failed: {error}"))?;
    #[cfg(windows)]
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("job write failed: {error}")
    })
}

/// All saved jobs by name, with the file stems that would not parse. Skips
/// rather than bricks, like the recipe library.
pub fn list(dir: &std::path::Path) -> (Vec<Job>, Vec<String>) {
    let mut jobs = Vec::new();
    let mut skipped = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (jobs, skipped);
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
            .filter(|job| job.id == stem)
        {
            Some(job) => jobs.push(job),
            None => skipped.push(path.display().to_string()),
        }
    }
    jobs.sort_by(|left, right| left.name.cmp(&right.name));
    (jobs, skipped)
}

/// Save a job, creating or replacing its file atomically. The revision is
/// the caller's: bump before saving a mutation.
pub fn save(dir: &std::path::Path, job: &Job) -> Result<(), String> {
    let pretty = serde_json::to_string_pretty(job)
        .map_err(|error| format!("job does not serialize: {error}"))?;
    parse_bytes(pretty.as_bytes())?;
    check_slug(&job.id, "job")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("job library failed: {error}"))?;
    let (jobs, _) = list(dir);
    if jobs.len() >= MAX_JOBS && !jobs.iter().any(|known| known.id == job.id) {
        return Err(format!("the job library holds at most {MAX_JOBS} jobs"));
    }
    write_atomic(&file_for(dir, &job.id), pretty.as_bytes())
}

/// Delete one job by id. Sources, outputs and recipes live elsewhere; only
/// the job file goes.
pub fn remove(dir: &std::path::Path, id: &str) -> Result<(), String> {
    check_slug(id, "job")?;
    std::fs::remove_file(file_for(dir, id)).map_err(|_| format!("no job named {id:?} exists"))
}

/// A file stem from a display name, deduplicated against the library.
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
        stem = "job".into();
    }
    let (jobs, _) = list(dir);
    let taken: std::collections::HashSet<&str> = jobs.iter().map(|job| job.id.as_str()).collect();
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

/// One mapped file as the product view sees it: where it is, and whether it
/// is still what the mapping recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappedSource {
    pub mapping_id: String,
    pub path: std::path::PathBuf,
    pub status: SourceStatus,
}

/// Freshness of one mapped file against the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceStatus {
    /// On disk with the recorded size and mtime.
    Fresh,
    /// On disk, but the size or mtime moved: revalidate before trusting it.
    Stale,
    /// Gone: relink, do not guess.
    Missing,
}

/// One role's resolution: its mapped files with freshness, or the reason it
/// has none. Required comes from the role; the status never invents it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleState {
    pub product_id: String,
    pub role_id: String,
    pub required: bool,
    pub status: RoleStatus,
    pub sources: Vec<MappedSource>,
}

/// Whether a role can be prepared from what is mapped right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleStatus {
    /// Every mapped file is fresh.
    Ready,
    /// At least one file changed under its mapping.
    Stale,
    /// At least one file is gone.
    Missing,
    /// Nothing mapped to this role yet.
    Unmapped,
}

fn file_status(source: &SourceRef) -> SourceStatus {
    let Ok(metadata) = std::fs::metadata(&source.path) else {
        return SourceStatus::Missing;
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs());
    if metadata.len() == source.bytes && modified == source.modified {
        SourceStatus::Fresh
    } else {
        SourceStatus::Stale
    }
}

/// Resolve every role of one product against the disk. Pure filesystem reads,
/// no writes, no network: safe to run whenever the view needs the truth.
pub fn resolve_product(product: &ProductSet) -> Vec<RoleState> {
    product
        .roles
        .iter()
        .map(|role| {
            let sources: Vec<MappedSource> = product
                .mappings
                .iter()
                .filter(|mapping| mapping.role_id == role.id)
                .map(|mapping| MappedSource {
                    mapping_id: mapping.id.clone(),
                    path: mapping.source.path.clone(),
                    status: file_status(&mapping.source),
                })
                .collect();
            let status = if sources.is_empty() {
                RoleStatus::Unmapped
            } else if sources
                .iter()
                .any(|source| source.status == SourceStatus::Missing)
            {
                RoleStatus::Missing
            } else if sources
                .iter()
                .any(|source| source.status == SourceStatus::Stale)
            {
                RoleStatus::Stale
            } else {
                RoleStatus::Ready
            };
            RoleState {
                product_id: product.id.clone(),
                role_id: role.id.clone(),
                required: role.required,
                status,
                sources,
            }
        })
        .collect()
}

/// Bind or clear the job's target recipe. The id is recorded, never applied:
/// preparation resolves it and refuses a missing one by name instead of
/// silently converting with whatever is selected.
pub fn bind_target(job: &mut Job, recipe_id: Option<&str>) {
    job.target_recipe = recipe_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
}

/// Resolve the bound target against known recipes. A bound id with no recipe
/// refuses with its name rather than falling back to current settings.
pub fn resolve_target<'a>(
    job: &'a Job,
    recipes: &'a [crate::recipe::Recipe],
) -> Result<Option<&'a crate::recipe::Recipe>, String> {
    match &job.target_recipe {
        None => Ok(None),
        Some(id) => recipes
            .iter()
            .find(|recipe| &recipe.id == id)
            .map(Some)
            .ok_or_else(|| format!("{id:?} is not saved as a recipe anymore")),
    }
}

/// Re-point one mapping at a file that exists, refreshing its recorded size
/// and mtime. The id stays: history follows the row, not the path.
pub fn relink(job: &mut Job, mapping_id: &str, path: &std::path::Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| format!("relink target {} is not a file", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("relink target {} is not a file", path.display()));
    }
    let mapping = job
        .products
        .iter_mut()
        .flat_map(|product| product.mappings.iter_mut())
        .find(|mapping| mapping.id == mapping_id)
        .ok_or_else(|| format!("no mapping named {mapping_id:?} exists"))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs());
    mapping.source = SourceRef {
        path: path.to_path_buf(),
        bytes: metadata.len(),
        modified,
    };
    Ok(())
}

/// Normalize a file stem for matching: lowercase, separators to one dash.
fn stem_key(name: &str) -> String {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    stem.to_lowercase()
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
        .join("-")
}

/// What filename-assisted matching found: exactly one file, several, or none.
/// Several is information, not an error: the caller shows the choice instead
/// of writing an ambiguous mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match {
    Exact(std::path::PathBuf),
    Ambiguous(Vec<std::path::PathBuf>),
    Missing,
}

/// Match one wanted filename against folder entries: an exact stem match wins;
/// otherwise every entry containing the wanted stem is a candidate.
pub fn match_filename(wanted: &str, entries: &[crate::scan::Entry]) -> Match {
    let key = stem_key(wanted);
    if key.is_empty() {
        return Match::Missing;
    }
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for entry in entries {
        let candidate = stem_key(&entry.name());
        if candidate == key {
            exact.push(entry.path.clone());
        } else if candidate.contains(&key) || key.contains(&candidate) {
            partial.push(entry.path.clone());
        }
    }
    match (exact.len(), partial.is_empty()) {
        (1, _) => Match::Exact(exact.remove(0)),
        (0, true) => Match::Missing,
        _ => {
            let mut candidates = exact;
            candidates.extend(partial);
            candidates.sort();
            candidates.dedup();
            Match::Ambiguous(candidates)
        }
    }
}

/// One parsed mapping row and what happened to it. Only `Mapped` writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CsvOutcome {
    Mapped {
        product_id: String,
        role_id: String,
        path: std::path::PathBuf,
    },
    AmbiguousSku {
        sku: String,
    },
    UnknownProduct {
        sku: String,
    },
    UnknownRole {
        product_id: String,
        role: String,
    },
    AmbiguousFile {
        filename: String,
        candidates: Vec<std::path::PathBuf>,
    },
    MissingFile {
        filename: String,
    },
}

/// Split one CSV line honoring double quotes (`""` escapes one). Enough for
/// mapping sheets; not a general CSV library, and documented as such.
fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(cell) = chars.next() {
        match cell {
            '"' if !quoted => quoted = true,
            '"' => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            ',' if !quoted => {
                fields.push(std::mem::take(&mut field));
            }
            _ => field.push(cell),
        }
    }
    fields.push(field);
    fields
}

/// Map files from CSV text with `sku,role,filename` rows. A header row naming
/// those columns is accepted and skipped; `#` lines and blanks are ignored.
/// Exact single file matches write mappings with fresh ids; everything else
/// is reported per row and writes nothing.
pub fn apply_csv(job: &mut Job, text: &str, entries: &[crate::scan::Entry]) -> Vec<CsvOutcome> {
    let mut outcomes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = split_csv(line);
        let row: Vec<&str> = fields.iter().map(|field| field.trim()).collect();
        if row.len() == 3
            && row[0].eq_ignore_ascii_case("sku")
            && row[1].eq_ignore_ascii_case("role")
            && row[2].eq_ignore_ascii_case("filename")
        {
            continue;
        }
        let [sku, role, filename] = row.as_slice() else {
            continue;
        };
        let mut products = job
            .products
            .iter()
            .filter(|product| product.sku_hint == *sku || product.id == *sku);
        let Some(first) = products.next() else {
            outcomes.push(CsvOutcome::UnknownProduct {
                sku: sku.to_string(),
            });
            continue;
        };
        if products.next().is_some() {
            outcomes.push(CsvOutcome::AmbiguousSku {
                sku: sku.to_string(),
            });
            continue;
        }
        let product_id = first.id.clone();
        let role_known = first.roles.iter().any(|known| known.id == *role);
        if !role_known {
            outcomes.push(CsvOutcome::UnknownRole {
                product_id,
                role: role.to_string(),
            });
            continue;
        }
        match match_filename(filename, entries) {
            Match::Missing => outcomes.push(CsvOutcome::MissingFile {
                filename: filename.to_string(),
            }),
            Match::Ambiguous(candidates) => outcomes.push(CsvOutcome::AmbiguousFile {
                filename: filename.to_string(),
                candidates,
            }),
            Match::Exact(path) => {
                let product = job
                    .products
                    .iter_mut()
                    .find(|product| product.id == product_id)
                    .expect("the product resolved above");
                if product
                    .mappings
                    .iter()
                    .any(|mapping| mapping.role_id == *role && mapping.source.path == path)
                {
                    continue;
                }
                let mut counter = product.mappings.len() + 1;
                while product
                    .mappings
                    .iter()
                    .any(|mapping| mapping.id == format!("m{counter}"))
                {
                    counter += 1;
                }
                let metadata = std::fs::metadata(&path).expect("matched files exist");
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|age| age.as_secs());
                let id = format!("m{counter}");
                product.mappings.push(Mapping {
                    id: id.clone(),
                    role_id: role.to_string(),
                    source: SourceRef {
                        path: path.clone(),
                        bytes: metadata.len(),
                        modified,
                    },
                });
                outcomes.push(CsvOutcome::Mapped {
                    product_id,
                    role_id: role.to_string(),
                    path,
                });
            }
        }
    }
    outcomes
}

/// One deliverable the folder still owes an explanation for: which output came
/// from which mapped source, and why it no longer matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleDeliverable {
    pub product_id: String,
    pub role_id: String,
    pub mapping_id: String,
    pub output: std::path::PathBuf,
    pub reason: StaleReason,
}

/// Why a recorded output stopped describing its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaleReason {
    /// The source changed size or mtime since the mapping that built this.
    SourceChanged,
    /// The output is gone or somebody rewrote it since the run.
    OutputGone,
}

/// Which recorded outputs no longer match their mapped sources. Reads the run
/// manifest beside the outputs; plans nothing, writes nothing. `root` scopes
/// the mappings, `out_dir` the recorded outputs; anything outside either is
/// skipped.
pub fn stale_deliverables(
    manifest: &crate::manifest::Manifest,
    job: &Job,
    root: &std::path::Path,
    out_dir: &std::path::Path,
) -> Vec<StaleDeliverable> {
    let mut stale = Vec::new();
    for product in &job.products {
        for state in resolve_product(product) {
            for source in &state.sources {
                let Ok(relative) = source.path.strip_prefix(root) else {
                    continue;
                };
                let outputs: Vec<&crate::manifest::Record> = manifest
                    .outputs
                    .iter()
                    .filter(|record| record.source.as_path() == relative)
                    .collect();
                if outputs.is_empty() {
                    continue;
                }
                let mapping = job
                    .products
                    .iter()
                    .find(|product| product.id == state.product_id)
                    .and_then(|product| {
                        product
                            .mappings
                            .iter()
                            .find(|mapping| mapping.id == source.mapping_id)
                    });
                let current = mapping.map_or(SourceStatus::Missing, |mapping| {
                    file_status(&mapping.source)
                });
                for record in outputs {
                    let output = out_dir.join(&record.output);
                    if !record.installed(&output) {
                        stale.push(StaleDeliverable {
                            product_id: state.product_id.clone(),
                            role_id: state.role_id.clone(),
                            mapping_id: source.mapping_id.clone(),
                            output,
                            reason: StaleReason::OutputGone,
                        });
                    } else if current != SourceStatus::Fresh {
                        stale.push(StaleDeliverable {
                            product_id: state.product_id.clone(),
                            role_id: state.role_id.clone(),
                            mapping_id: source.mapping_id.clone(),
                            output,
                            reason: StaleReason::SourceChanged,
                        });
                    }
                }
            }
        }
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn store(name: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "press-jobs-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn product() -> ProductSet {
        ProductSet {
            id: "hero".into(),
            name: "Hero".into(),
            sku_hint: "SKU-1".into(),
            roles: vec![
                Role {
                    id: "main".into(),
                    label: "Main".into(),
                    required: true,
                },
                Role {
                    id: "detail".into(),
                    label: "Detail".into(),
                    required: false,
                },
            ],
            mappings: vec![Mapping {
                id: "m1".into(),
                role_id: "main".into(),
                source: SourceRef {
                    path: PathBuf::from("/photos/hero.png"),
                    bytes: 100,
                    modified: Some(7),
                },
            }],
            binding: None,
        }
    }

    fn job() -> Job {
        Job {
            schema: SCHEMA_VERSION,
            id: "job".into(),
            name: "Job".into(),
            revision: 2,
            source_roots: vec![PathBuf::from("/photos")],
            target_recipe: Some("night".into()),
            products: vec![product()],
        }
    }

    #[test]
    fn jobs_round_trip_and_reject_strangers() {
        let json = serde_json::to_string(&job()).unwrap();
        assert_eq!(parse_bytes(json.as_bytes()).unwrap(), job());
        let mut future = job();
        future.schema = SCHEMA_VERSION + 1;
        assert!(parse_bytes(&serde_json::to_vec(&future).unwrap()).is_err());
        let unknown = json.replace("\"revision\":2", "\"revision\":2,\"color\":\"red\"");
        assert!(
            parse_bytes(unknown.as_bytes()).is_err(),
            "unknown fields refuse"
        );
        let mut orphan = job();
        orphan.products[0].mappings[0].role_id = "ghost".into();
        assert!(parse_bytes(&serde_json::to_vec(&orphan).unwrap()).is_err());
        let mut dup = job();
        dup.products.push(product());
        assert!(parse_bytes(&serde_json::to_vec(&dup).unwrap()).is_err());
    }

    #[test]
    fn save_list_remove_round_trip() {
        let dir = store("round-trip");
        save(&dir, &job()).unwrap();
        let (listed, skipped) = list(&dir);
        assert!(skipped.is_empty());
        assert_eq!(listed, vec![job()]);
        assert!(remove(&dir, "job").is_ok());
        assert!(remove(&dir, "job").is_err(), "deleting twice fails");
        assert!(list(&dir).0.is_empty());
        // `dir` is absolute on every platform; a literal `/p` is not one on
        // Windows, where a root without a prefix is not an absolute path.
        assert!(
            save(
                &dir,
                &Job::new("x".into(), "X".into(), vec![dir.join("p")]).unwrap()
            )
            .is_ok()
        );
        assert!(Job::new("Bad Id!".into(), "X".into(), vec![dir.join("p")]).is_err());
        assert!(
            Job::new("x".into(), "X".into(), vec![]).is_err(),
            "roots are required"
        );
        assert!(Job::new("x".into(), "X".into(), vec![PathBuf::from("relative")]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn portable_export_rebases_and_never_guesses() {
        let dir = store("portable");
        let root = dir.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("hero.png");
        std::fs::write(&file, b"pixels").unwrap();
        let metadata = std::fs::metadata(&file).unwrap();
        let mut job = job();
        job.source_roots = vec![root.clone()];
        job.products[0].mappings[0].source.path = file;
        job.products[0].mappings[0].source.bytes = metadata.len();
        let portable = job.to_portable(&root).unwrap();
        assert_eq!(
            portable.products[0].mappings[0].source.path,
            PathBuf::from("hero.png")
        );
        let back = Job::from_portable(&portable, &root).unwrap();
        // Rebased onto the same root, the job is itself.
        let mut expected = job.clone();
        expected.products[0].mappings[0].source.modified =
            back.products[0].mappings[0].source.modified;
        assert_eq!(back.products[0].mappings, expected.products[0].mappings);
        // An outside path refuses instead of silently dropping the file.
        let mut stray = job.clone();
        stray.products[0].mappings[0].source.path = PathBuf::from("/elsewhere/x.png");
        assert!(stray.to_portable(&root).is_err());
        // A non-relative portable path refuses at import: absolute, rooted,
        // and parent-escaping alike, since joining any of them would land
        // outside the root its new owner chose.
        let mut evil = portable;
        evil.products[0].mappings[0].source.path = PathBuf::from("/abs.png");
        assert!(Job::from_portable(&evil, &root).is_err());
        for escape in ["../escape.png", "sub/../../escape.png"] {
            evil.products[0].mappings[0].source.path = PathBuf::from(escape);
            assert!(
                Job::from_portable(&evil, &root).is_err(),
                "{escape} must not escape the import root"
            );
        }
        // Equal SKUs for different recipients stay distinct products.
        let mut two = job.clone();
        let mut sibling = product();
        sibling.id = "hero-2".into();
        sibling.mappings.clear();
        two.products.push(sibling);
        let portable = two.to_portable(&root).unwrap();
        assert_eq!(portable.products.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn entry(path: &std::path::Path) -> crate::scan::Entry {
        crate::scan::Entry {
            path: path.to_path_buf(),
            format: image::ImageFormat::Png.into(),
            width: 8,
            height: 8,
            bytes: std::fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        }
    }

    fn mapped_job(dir: &std::path::Path) -> (Job, std::path::PathBuf, std::path::PathBuf) {
        let fresh = dir.join("fresh.png");
        let stale = dir.join("stale.png");
        std::fs::write(&fresh, b"fresh-bytes").unwrap();
        std::fs::write(&stale, b"stale").unwrap();
        let mut job = job();
        job.source_roots = vec![dir.to_path_buf()];
        job.products[0].mappings.clear();
        for (id, path) in [("m1", fresh.clone()), ("m2", stale.clone())] {
            let metadata = std::fs::metadata(&path).unwrap();
            job.products[0].mappings.push(Mapping {
                id: id.into(),
                role_id: "main".into(),
                source: SourceRef {
                    path: path.clone(),
                    bytes: metadata.len(),
                    modified: metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|age| age.as_secs()),
                },
            });
        }
        // The stale file outgrows its recorded size after mapping.
        std::fs::write(&stale, b"stale-now-much-longer").unwrap();
        (job, fresh, stale)
    }

    #[test]
    fn bound_targets_resolve_and_missing_ones_refuse_by_name() {
        let mut job = job();
        bind_target(&mut job, None);
        assert_eq!(resolve_target(&job, &[]).unwrap(), None);
        bind_target(&mut job, Some("  night  "));
        assert_eq!(job.target_recipe.as_deref(), Some("night"));
        bind_target(&mut job, Some("   "));
        assert_eq!(job.target_recipe, None);
        bind_target(&mut job, Some("night"));
        let night = crate::recipe::Recipe {
            schema: crate::recipe::SCHEMA_VERSION,
            id: "night".into(),
            name: "Night".into(),
            revision: 2,
            provenance: crate::recipe::Provenance::Personal,
            format: crate::recipe::RecipeFormat::WebP,
            quality: crate::recipe::RecipeQuality::Lossy(80.),
            max_edge: None,
            avif_speed: None,
        };
        assert_eq!(
            resolve_target(&job, &[night])
                .unwrap()
                .map(|recipe| &recipe.name),
            Some(&"Night".to_string())
        );
        let missing = resolve_target(&job, &[]).unwrap_err();
        assert!(missing.contains("night"), "the refusal names it: {missing}");
    }

    #[test]
    fn filename_matching_is_exact_first_and_honest_otherwise() {
        let dir = store("match");
        for name in [
            "hero-shot.png",
            "red-hero.png",
            "blue-hero.png",
            "detail.png",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let entries: Vec<crate::scan::Entry> = [
            "hero-shot.png",
            "red-hero.png",
            "blue-hero.png",
            "detail.png",
        ]
        .iter()
        .map(|name| entry(&dir.join(name)))
        .collect();
        assert_eq!(
            match_filename("detail.png", &entries),
            Match::Exact(dir.join("detail.png"))
        );
        assert_eq!(
            match_filename("Hero Shot", &entries),
            Match::Exact(dir.join("hero-shot.png")),
            "one exact stem wins no matter the separators"
        );
        assert_eq!(
            match_filename("hero", &entries),
            Match::Ambiguous(vec![
                dir.join("blue-hero.png"),
                dir.join("hero-shot.png"),
                dir.join("red-hero.png"),
            ]),
            "three partials choose nothing"
        );
        assert_eq!(match_filename("missing.png", &entries), Match::Missing);
        assert_eq!(match_filename("", &entries), Match::Missing);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolution_names_fresh_stale_missing_and_unmapped() {
        let dir = store("resolve");
        let (job, _, _) = mapped_job(&dir);
        let states = resolve_product(&job.products[0]);
        assert_eq!(states.len(), 2);
        let detail = states
            .iter()
            .find(|state| state.role_id == "detail")
            .unwrap();
        assert_eq!(detail.status, RoleStatus::Unmapped);
        assert!(detail.sources.is_empty());
        let main = states.iter().find(|state| state.role_id == "main").unwrap();
        assert_eq!(main.status, RoleStatus::Stale);
        assert_eq!(main.sources.len(), 2);
        std::fs::remove_file(dir.join("fresh.png")).unwrap();
        let main = resolve_product(&job.products[0])
            .into_iter()
            .find(|state| state.role_id == "main")
            .unwrap();
        assert_eq!(main.status, RoleStatus::Missing);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relink_refreshes_identity_and_keeps_the_row() {
        let dir = store("relink");
        let (mut job, _, _) = mapped_job(&dir);
        let target = dir.join("relinked.png");
        assert!(relink(&mut job, "nope", &target).is_err());
        assert!(relink(&mut job, "m1", &dir.join("gone.png")).is_err());
        std::fs::write(&target, b"new-bytes").unwrap();
        relink(&mut job, "m1", &target).unwrap();
        let states = resolve_product(&job.products[0]);
        let main = states.iter().find(|state| state.role_id == "main").unwrap();
        assert!(
            main.sources
                .iter()
                .any(|source| source.mapping_id == "m1" && source.status == SourceStatus::Fresh)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn csv_maps_exact_rows_and_reports_the_rest() {
        let dir = store("csv");
        for name in ["detail.png", "red-hero.png", "blue-hero.png"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let entries: Vec<crate::scan::Entry> = ["detail.png", "red-hero.png", "blue-hero.png"]
            .iter()
            .map(|name| entry(&dir.join(name)))
            .collect();
        let mut job = job();
        job.source_roots = vec![dir.clone()];
        job.products[0].mappings.clear();
        let outcomes = apply_csv(
            &mut job,
            "# comment\nsku,role,filename\n\nSKU-1,main,detail.png\nSKU-1,detail,hero\nSKU-1,main,missing.png\nNOPE,main,detail.png\nSKU-1,ghost,detail.png\n",
            &entries,
        );
        assert_eq!(outcomes.len(), 5);
        assert!(matches!(outcomes[0], CsvOutcome::Mapped { .. }));
        assert!(matches!(outcomes[1], CsvOutcome::AmbiguousFile { .. }));
        assert!(matches!(outcomes[2], CsvOutcome::MissingFile { .. }));
        assert!(matches!(outcomes[3], CsvOutcome::UnknownProduct { .. }));
        assert!(matches!(outcomes[4], CsvOutcome::UnknownRole { .. }));
        assert_eq!(
            job.products[0].mappings.len(),
            1,
            "only the exact row writes"
        );
        let outcomes = apply_csv(&mut job, "SKU-1,main,detail.png\n", &entries);
        assert!(outcomes.is_empty(), "an idempotent re-import stays silent");
        // Equal SKUs stay distinct products: the shared SKU refuses, either id maps.
        let mut two = job.clone();
        let mut sibling = product();
        sibling.id = "hero-2".into();
        two.products.push(sibling);
        let outcomes = apply_csv(&mut two, "SKU-1,detail,detail.png\n", &entries);
        assert!(matches!(outcomes[0], CsvOutcome::AmbiguousSku { .. }));
        let outcomes = apply_csv(&mut two, "hero-2,detail,detail.png\n", &entries);
        assert!(matches!(outcomes[0], CsvOutcome::Mapped { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deliverable_staleness_follows_source_and_output() {
        let dir = store("stale");
        let root = dir.join("photos");
        let out_dir = dir.join("optimized");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&out_dir).unwrap();
        let source = root.join("hero.png");
        std::fs::write(&source, b"v1").unwrap();
        let output = out_dir.join("hero.webp");
        std::fs::write(&output, b"encoded").unwrap();
        let manifest = crate::manifest::Manifest {
            outputs: vec![
                crate::manifest::Stamp::new(
                    crate::convert::Format::WebP,
                    crate::convert::Quality::lossy(80.),
                    crate::convert::MaxEdge::FULL,
                )
                .record((&root, &out_dir), &source, &output, &output, None)
                .expect("the run records"),
            ],
            rejected: Vec::new(),
        };
        let mut job = job();
        job.source_roots = vec![root.clone()];
        job.products[0].mappings.clear();
        let metadata = std::fs::metadata(&source).unwrap();
        job.products[0].mappings.push(Mapping {
            id: "m1".into(),
            role_id: "main".into(),
            source: SourceRef {
                path: source.clone(),
                bytes: metadata.len(),
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|age| age.as_secs()),
            },
        });
        assert!(stale_deliverables(&manifest, &job, &root, &out_dir).is_empty());
        std::fs::write(&source, b"version two, longer").unwrap();
        let stale = stale_deliverables(&manifest, &job, &root, &out_dir);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].reason, StaleReason::SourceChanged);
        std::fs::write(&output, b"somebody rewrote me!").unwrap();
        let stale = stale_deliverables(&manifest, &job, &root, &out_dir);
        assert!(
            stale
                .iter()
                .any(|stale| stale.reason == StaleReason::OutputGone),
            "a rewritten output no longer matches its record"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
