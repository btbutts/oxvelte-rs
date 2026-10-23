//! Generates `known_css_properties.json` — the merged, deduplicated list of
//! known CSS properties consumed by `no_unknown_style_directive_property.rs`
//!
//!
//! Flow:
//!   1. If the cached copy at `resources/css/known_css_properties.json`
//!      exists, copy it to `OUT_DIR` and stop (this is the release-tarball
//!      path — fully offline).
//!   2. Otherwise drive one loop over `css_prop_sources()`: fetch each pinned
//!      GitHub repo archive into a throwaway tmp dir, extract only the JSON we
//!      need, dedupe into a shared set, delete the archive, next source.
//!   3. Apply the merge rules (drop meta entries, lowercase, sort) and write
//!      the result to both the cache dir and `OUT_DIR`.
//!

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which archive format to request from GitHub. Only `Tarball` is used today,
/// but the `Zip` arm is implemented so the helper supports both suffixes.
#[derive(Clone, Copy)]
#[allow(dead_code)] // `Zip` is wired end-to-end and reserved for future use
enum ArchiveKind {
    Tarball,
    Zip,
}

impl ArchiveKind {
    fn ext(self) -> &'static str {
        match self {
            ArchiveKind::Tarball => ".tar.gz",
            ArchiveKind::Zip => ".zip",
        }
    }
}

/// How a source is downloaded: a full repo archive, or a single raw file
/// (for repos whose archives GitHub are too large to generate)
enum Fetch {
    Archive(ArchiveKind),
    RawFile(&'static str),
}

/// How to parse property names out of one JSON document.
#[derive(Clone, Copy)]
enum Schema {
    /// `{ "properties": ["accent-color", ...] }`
    PropertiesArray,
    /// Property names are object keys — top-level (`None`) or under a key
    /// (`Some("properties")`).
    ObjectKeys(Option<&'static str>),
    /// `{ "properties": [ { "name": "margin-top", ... }, ... ] }`
    NamedObjects,
}

/// Which members of a source's archive carry property data.
enum Extract {
    /// One JSON file at this repo-relative path.
    SingleJson { member: &'static str, schema: Schema },
    /// Every `*.json` directly under this repo-relative directory.
    MultiJson { dir: &'static str, schema: Schema },
    /// The member file names themselves are the property names (BCD).
    MemberFileNames { dir: &'static str },
}

struct SourceSpec {
    owner: &'static str,
    repo: &'static str,
    commit_env: &'static str,
    fetch: Fetch,
    extract: Extract,
}

/// The six upstream CSS property catalogues, keyed by short name. Everything
/// needed to build each source's download URL lives here except the commit
/// hash, which is pinned per-source in `.cargo/config.toml`
fn css_prop_sources() -> HashMap<&'static str, SourceSpec> {
    HashMap::from([
        (
            "known-css-properties",
            SourceSpec {
                owner: "known-css",
                repo: "known-css-properties",
                commit_env: "OXVELTE_CSS_SRC_KNOWN_CSS_PROPERTIES_COMMIT",
                fetch: Fetch::Archive(ArchiveKind::Tarball),
                extract: Extract::SingleJson {
                    member: "data/all.json",
                    schema: Schema::PropertiesArray,
                },
            },
        ),
        (
            "mdn-browser-compat-data",
            SourceSpec {
                owner: "mdn",
                repo: "browser-compat-data",
                commit_env: "OXVELTE_CSS_SRC_MDN_BROWSER_COMPAT_DATA_COMMIT",
                fetch: Fetch::Archive(ArchiveKind::Tarball),
                extract: Extract::MemberFileNames {
                    dir: "css/properties/",
                },
            },
        ),
        (
            "csstree",
            SourceSpec {
                owner: "csstree",
                repo: "csstree",
                commit_env: "OXVELTE_CSS_SRC_CSSTREE_COMMIT",
                fetch: Fetch::Archive(ArchiveKind::Tarball),
                extract: Extract::SingleJson {
                    member: "data/patch.json",
                    schema: Schema::ObjectKeys(Some("properties")),
                },
            },
        ),
        (
            "mdn-data",
            SourceSpec {
                owner: "mdn",
                repo: "data",
                commit_env: "OXVELTE_CSS_SRC_MDN_DATA_COMMIT",
                fetch: Fetch::Archive(ArchiveKind::Tarball),
                extract: Extract::SingleJson {
                    member: "css/properties.json",
                    schema: Schema::ObjectKeys(None),
                },
            },
        ),
        (
            // GitHub refuses to generate archives for WebKit (HTTP 422 — repo
            // too large), so this source is a single raw-file GET.
            "webkit",
            SourceSpec {
                owner: "WebKit",
                repo: "WebKit",
                commit_env: "OXVELTE_CSS_SRC_WEBKIT_COMMIT",
                fetch: Fetch::RawFile("Source/WebCore/css/CSSProperties.json"),
                extract: Extract::SingleJson {
                    member: "Source/WebCore/css/CSSProperties.json",
                    schema: Schema::ObjectKeys(Some("properties")),
                },
            },
        ),
        (
            "w3c-webref",
            SourceSpec {
                owner: "w3c",
                repo: "webref",
                commit_env: "OXVELTE_CSS_SRC_W3C_WEBREF_COMMIT",
                fetch: Fetch::Archive(ArchiveKind::Tarball),
                extract: Extract::MultiJson {
                    dir: "ed/css/",
                    schema: Schema::NamedObjects,
                },
            },
        ),
    ])
}

fn http_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(concat!("oxvelte/", env!("CARGO_PKG_VERSION"), " build.rs"))
        .build();
    ureq::Agent::new_with_config(config)
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let mut response = http_agent().get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("HTTP {code} from {url}"),
        other => format!("{other} ({url})"),
    })?;
    response
        .body_mut()
        .with_config()
        .limit(u64::MAX)
        .read_to_vec()
        .map_err(|e| format!("reading body of {url}: {e}"))
}

/// Stream a URL straight to a file — archives can far exceed ureq's default
/// 10 MiB in-memory body limit, so they never land in memory whole.
fn http_download(url: &str, dest: &Path) -> Result<(), String> {
    let mut response = http_agent().get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("HTTP {code} from {url}"),
        other => format!("{other} ({url})"),
    })?;
    let mut reader = response.body_mut().as_reader();
    let mut file = fs::File::create(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|e| format!("downloading {url}: {e}"))?;
    Ok(())
}

/// Generic GitHub repo archive downloader. It holds no repo-specific data —
/// owner, repo, commit hash and archive kind are all supplied by the caller —
/// so it can fetch any GitHub repo archive, here or for future uses.
fn fetch_github_repo(
    owner: &str,
    repo: &str,
    commit_hash: &str,
    archive: ArchiveKind,
    dest_dir: &Path,
) -> Result<PathBuf, String> {
    let url = format!("https://github.com/{owner}/{repo}/archive/{commit_hash}{ext}", ext = archive.ext());
    let path = dest_dir.join(format!("{owner}-{repo}-{commit_hash}{ext}", ext = archive.ext()));
    http_download(&url, &path)?;
    Ok(path)
}

/// What to do with one archive member whose repo-relative path is `rel`.
enum MemberAction {
    Skip,
    /// The member's file name (minus `.json`) is itself a property name.
    UseFileName,
    /// Read and parse the member's JSON content.
    UseContent,
}

fn member_action(rel: &str, extract: &Extract) -> MemberAction {
    match extract {
        Extract::SingleJson { member, .. } if rel == *member => MemberAction::UseContent,
        Extract::MultiJson { dir, .. }
            if rel.starts_with(dir)
                && rel.ends_with(".json")
                && !rel[dir.len()..].contains('/') =>
        {
            MemberAction::UseContent
        }
        Extract::MemberFileNames { dir }
            if rel.starts_with(dir)
                && rel.ends_with(".json")
                && !rel[dir.len()..].contains('/') =>
        {
            MemberAction::UseFileName
        }
        _ => MemberAction::Skip,
    }
}

/// Strip the archive's single top-level `<repo>-<hash>/` wrapper directory.
fn strip_archive_root(path: &Path) -> Option<String> {
    let mut components = path.components();
    components.next()?;
    let rel = components.as_path().to_str()?.replace('\\', "/");
    (!rel.is_empty()).then_some(rel)
}

fn parse_property_json(bytes: &[u8], schema: Schema, source: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .unwrap_or_else(|e| panic!("CSS source '{source}': invalid JSON: {e}"));
    let missing = || -> ! { panic!("CSS source '{source}': JSON does not match the expected schema") };
    match schema {
        Schema::PropertiesArray => value["properties"]
            .as_array()
            .unwrap_or_else(|| missing())
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect(),
        Schema::ObjectKeys(key) => {
            let target = match key {
                Some(key) => &value[key],
                None => &value,
            };
            target
                .as_object()
                .unwrap_or_else(|| missing())
                .keys()
                .cloned()
                .collect()
        }
        Schema::NamedObjects => value["properties"]
            .as_array()
            .unwrap_or_else(|| missing())
            .iter()
            .filter_map(|p| p["name"].as_str().map(str::to_string))
            .collect(),
    }
}

fn extract_from_tarball(archive: &Path, spec: &SourceSpec, key: &str) -> Vec<String> {
    let fail = |e: &dyn std::fmt::Display| -> ! {
        panic!("CSS source '{key}': tarball extraction failed ({e}) — upstream layout likely changed")
    };
    let file = fs::File::open(archive).unwrap_or_else(|e| fail(&e));
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut names = Vec::new();
    let mut found_single = false;
    for entry in tar.entries().unwrap_or_else(|e| fail(&e)) {
        let mut entry = entry.unwrap_or_else(|e| fail(&e));
        let path = entry.path().unwrap_or_else(|e| fail(&e)).into_owned();
        let Some(rel) = strip_archive_root(&path) else { continue };
        match member_action(&rel, &spec.extract) {
            MemberAction::Skip => {}
            MemberAction::UseFileName => {
                names.push(rel.rsplit('/').next().unwrap().trim_end_matches(".json").to_string());
            }
            MemberAction::UseContent => {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap_or_else(|e| fail(&e));
                let schema = match &spec.extract {
                    Extract::SingleJson { schema, .. } | Extract::MultiJson { schema, .. } => *schema,
                    Extract::MemberFileNames { .. } => unreachable!(),
                };
                names.extend(parse_property_json(&bytes, schema, key));
                found_single = true;
            }
        }
    }
    if let Extract::SingleJson { member, .. } = spec.extract {
        assert!(found_single, "CSS source '{key}': member '{member}' not found in tarball — upstream layout likely changed");
    }
    names
}

fn extract_from_zip(archive: &Path, spec: &SourceSpec, key: &str) -> Vec<String> {
    let fail = |e: &dyn std::fmt::Display| -> ! {
        panic!("CSS source '{key}': zip extraction failed ({e}) — upstream layout likely changed")
    };
    let file = fs::File::open(archive).unwrap_or_else(|e| fail(&e));
    let mut zip = zip::ZipArchive::new(file).unwrap_or_else(|e| fail(&e));
    let mut names = Vec::new();
    let mut found_single = false;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap_or_else(|e| fail(&e));
        let name = entry.name().unwrap_or_else(|e| fail(&e)).into_owned();
        let Some(rel) = strip_archive_root(Path::new(&name)) else { continue };
        match member_action(&rel, &spec.extract) {
            MemberAction::Skip => {}
            MemberAction::UseFileName => {
                names.push(rel.rsplit('/').next().unwrap().trim_end_matches(".json").to_string());
            }
            MemberAction::UseContent => {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap_or_else(|e| fail(&e));
                let schema = match &spec.extract {
                    Extract::SingleJson { schema, .. } | Extract::MultiJson { schema, .. } => *schema,
                    Extract::MemberFileNames { .. } => unreachable!(),
                };
                names.extend(parse_property_json(&bytes, schema, key));
                found_single = true;
            }
        }
    }
    if let Extract::SingleJson { member, .. } = spec.extract {
        assert!(found_single, "CSS source '{key}': member '{member}' not found in zip — upstream layout likely changed");
    }
    names
}

/// Fetch one source and return its property names. Download problems are
/// `Err` (the caller warns and continues); extraction or parse problems panic.
fn fetch_source(key: &str, spec: &SourceSpec, tmp_dir: &Path) -> Result<Vec<String>, String> {
    let commit = std::env::var(spec.commit_env).map_err(|_| {
        format!("env var {} is not set — define it in .cargo/config.toml", spec.commit_env)
    })?;
    match &spec.fetch {
        Fetch::RawFile(path) => {
            let url = format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/{commit}/{path}",
                owner = spec.owner,
                repo = spec.repo,
            );
            let bytes = http_get(&url)?;
            let schema = match &spec.extract {
                Extract::SingleJson { schema, .. } => *schema,
                _ => unreachable!("RawFile sources use SingleJson extraction"),
            };
            Ok(parse_property_json(&bytes, schema, key))
        }
        Fetch::Archive(kind) => {
            let archive = fetch_github_repo(spec.owner, spec.repo, &commit, *kind, tmp_dir)?;
            let names = match kind {
                ArchiveKind::Tarball => extract_from_tarball(&archive, spec, key),
                ArchiveKind::Zip => extract_from_zip(&archive, spec, key),
            };
            let _ = fs::remove_file(&archive);
            Ok(names)
        }
    }
}

/// Merge rules from the design doc: lowercase, and drop the `--*`
/// custom-property wildcard and other non-property meta entries.
fn normalize_property(raw: &str) -> Option<String> {
    let prop = raw.trim().to_lowercase();
    if prop.is_empty()
        || prop.starts_with("--")
        || prop.contains(char::is_whitespace)
    {
        None
    } else {
        Some(prop)
    }
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let cache = manifest_dir.join("resources/css/known_css_properties.json");
    let out = out_dir.join("known_css_properties.json");

    let sources = css_prop_sources();
    println!("cargo:rerun-if-changed={}", cache.display());
    println!("cargo:rerun-if-env-changed=OXVELTE_BUILD_SHOW_CSS_PROP_GEN_STATS");
    for spec in sources.values() {
        println!("cargo:rerun-if-env-changed={}", spec.commit_env);
    }

    // Cached copy present: release tarballs and repeat builds take this
    // fully offline path.
    if cache.exists() {
        fs::copy(&cache, &out).expect("copy cached known_css_properties.json into OUT_DIR");
        return;
    }

    // Success chatter is opt-in (`.cargo/config.toml`); genuine failures
    // always warn.
    let show_stats = std::env::var("OXVELTE_BUILD_SHOW_CSS_PROP_GEN_STATS")
        .is_ok_and(|v| matches!(v.as_str(), "true" | "1"));

    let tmp_dir = std::env::temp_dir().join(format!("oxvelte-css-props-{}", std::process::id()));
    fs::create_dir_all(&tmp_dir).expect("create temp dir for CSS source downloads");

    let mut properties = BTreeSet::new();
    let mut succeeded = 0;
    for (key, spec) in &sources {
        match fetch_source(key, spec, &tmp_dir) {
            Ok(names) => {
                let fetched = names.len();
                let before = properties.len();
                properties.extend(names.into_iter().filter_map(|n| normalize_property(&n)));
                if show_stats {
                    println!(
                        "cargo::warning=CSS property list generation: \
                        Source repo '{key}' yielded {fetched} properties, {} new after dedupe",
                        properties.len() - before
                    );
                }
                succeeded += 1;
            }
            Err(reason) => {
                println!(
                    "cargo::warning=CSS property list generation: \
                    Source repo '{key}' download failed ({reason}); continuing without it"
                );
            }
        }
    }
    let _ = fs::remove_dir(&tmp_dir);

    assert!(
        !properties.is_empty(),
        "all CSS property sources failed to download — cannot generate known_css_properties.json"
    );
    if succeeded < sources.len() {
        println!(
            "cargo::warning=CSS property list generation: \
            Generated from {succeeded}/{} sources; The list may be missing properties so \
            you may want to review resources/css/known_css_properties.json",
            sources.len()
        );
    }

    let json = serde_json::to_string_pretty(&serde_json::json!({ "properties": properties }))
        .expect("serialize known_css_properties.json");
    fs::create_dir_all(cache.parent().unwrap()).expect("create resources/css/");
    fs::write(&cache, &json).expect("write resources/css/known_css_properties.json");
    fs::write(&out, json).expect("write known_css_properties.json to OUT_DIR");
    if show_stats {
        println!(
            "cargo::warning=CSS property list generation: \
            Created known_css_properties.json with {} properties from {succeeded} sources",
            properties.len()
        );
    }
}
