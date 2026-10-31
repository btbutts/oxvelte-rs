//! Locating the external `tsrs` binary and detecting a project's TypeScript
//! major version. Resolution runs on every call and is never cached, so a
//! project-level install added between runs is picked up automatically.
//! See `docs/research/dependency/tsrs-type-aware-checking.md`
//! ("Distribution design").

use std::path::{Path, PathBuf};

/// Binary names we accept, in order. `.cmd` covers npm/bun shims on Windows.
const BINARY_NAMES: [&str; 3] = ["tsrs", "tsrs.exe", "tsrs.cmd"];

/// Resolves the `tsrs` binary to drive for a component: the project's own
/// `node_modules/.bin/tsrs` walked up from the linted file first (a
/// project-level install always wins), then `PATH`.
pub fn find_tsrs_binary(from_file: &Path) -> Option<PathBuf> {
    for dir in from_file.parent()?.ancestors() {
        for name in BINARY_NAMES {
            let candidate = dir.join("node_modules").join(".bin").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    find_on_path()
}

pub fn find_on_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in BINARY_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The major version of the project's own TypeScript install: the nearest
/// `node_modules/typescript/package.json` walked up from the linted file —
/// the same anchor the JS sidecar's `createRequire` uses. Returns `None`
/// when the project has no typescript install (the JS sidecar path already
/// errors in that case).
pub fn project_ts_major_version(from_file: &Path) -> Option<u64> {
    for dir in from_file.parent()?.ancestors() {
        let manifest = dir.join("node_modules").join("typescript").join("package.json");
        if manifest.is_file() {
            let text = std::fs::read_to_string(&manifest).ok()?;
            let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
            let version = parsed["version"].as_str()?;
            return version.split('.').next()?.parse().ok();
        }
    }
    None
}

/// True when the project is eligible for tsrs routing: TypeScript 6 or
/// newer (TS6 is the bridge release, type-system-continuous with TS7).
pub fn is_tsrs_eligible(from_file: &Path) -> bool {
    project_ts_major_version(from_file).is_some_and(|major| major >= 6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_project(name: &str) -> PathBuf {
        // Tests run in parallel threads within one process, so the pid alone
        // does not make the scratch dir unique.
        let root = std::env::temp_dir().join(format!("oxvelte-tsrs-resolve-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn project_install_wins_over_deeper_directories() {
        let root = make_project("outer");
        let bin = root.join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("tsrs"), "#!/bin/sh\n").unwrap();
        let nested = root.join("packages/app/src");
        fs::create_dir_all(&nested).unwrap();

        let found = find_tsrs_binary(&nested.join("component.svelte")).unwrap();
        assert_eq!(found, bin.join("tsrs"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deeper_project_install_wins_over_outer_one() {
        let root = make_project("nested");
        let outer = root.join("node_modules/.bin");
        let inner = root.join("packages/app/node_modules/.bin");
        fs::create_dir_all(&outer).unwrap();
        fs::create_dir_all(&inner).unwrap();
        fs::write(outer.join("tsrs"), "outer").unwrap();
        fs::write(inner.join("tsrs"), "inner").unwrap();

        let found = find_tsrs_binary(&root.join("packages/app/src/component.svelte")).unwrap();
        assert_eq!(found, inner.join("tsrs"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ts_major_version_from_nearest_typescript_install() {
        let root = make_project("version");
        let ts = root.join("node_modules/typescript");
        fs::create_dir_all(&ts).unwrap();
        fs::write(ts.join("package.json"), r#"{"name":"typescript","version":"6.1.2"}"#).unwrap();

        assert_eq!(project_ts_major_version(&root.join("src/component.svelte")), Some(6));
        assert!(is_tsrs_eligible(&root.join("src/component.svelte")));

        fs::write(ts.join("package.json"), r#"{"name":"typescript","version":"5.9.3"}"#).unwrap();
        assert!(!is_tsrs_eligible(&root.join("src/component.svelte")));

        fs::write(
            ts.join("package.json"),
            r#"{"name":"typescript","version":"7.1.0-beta"}"#,
        )
        .unwrap();
        assert_eq!(project_ts_major_version(&root.join("src/component.svelte")), Some(7));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_typescript_install_is_not_eligible() {
        let root = make_project("none");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(project_ts_major_version(&root.join("src/component.svelte")), None);
        assert!(!is_tsrs_eligible(&root.join("src/component.svelte")));
        let _ = fs::remove_dir_all(&root);
    }
}
