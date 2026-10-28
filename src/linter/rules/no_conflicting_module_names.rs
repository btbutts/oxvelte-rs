//! `svelte/no-conflicting-module-names` — disallow a `.svelte` component and
//! a same-named runes module (e.g. `Foo.svelte` and `Foo.svelte.ts`) from
//! coexisting in one directory. Bundlers resolve `./Foo.svelte` to the
//! component, but plain TypeScript appends `.ts` and lands on the module.
//!
//! This rule is filesystem-driven: it inspects sibling file *names*, not the
//! linted file's contents. The pure [`sibling_conflict`] check is shared with
//! the future `notify`-driven watch worker — see
//! `docs/research/test/no-conflicting-module-names.md`.

use crate::linter::{LintContext, Rule};
use oxc::span::Span;
use std::path::Path;

/// Extensions that plain TypeScript resolution appends to an unknown
/// `.svelte` specifier. `.d.ts` is intentionally absent: `Foo.svelte.d.ts`
/// is a hand-written component declaration and does not shadow the
/// component (stripping `.ts` leaves `Foo.svelte.d`, not a component).
pub const MODULE_EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"];

/// Which side of a same-name collision a linted file is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// The linted file is the `Foo.svelte` component; a sibling module
    /// exists. Carries the module's real on-disk basename.
    OnComponent { module_name: String },
    /// The linted file is a `Foo.svelte.<ext>` module; the sibling
    /// component exists. Carries the component's real on-disk basename.
    OnModule { svelte_name: String },
}

/// Check whether `path` is one side of a component/module name collision.
/// Pure apart from filesystem stat/read_dir calls — the lint-time trigger
/// (`Rule::run`) and the future watch worker both call this.
pub fn sibling_conflict(path: &Path) -> Option<Conflict> {
    let file_name = path.file_name()?.to_str()?;
    if !path.is_file() {
        return None;
    }

    // Component side: `Foo.svelte` collides with `Foo.svelte.<ext>`.
    if file_name.ends_with(".svelte") {
        for ext in MODULE_EXTENSIONS {
            let module_path = path.with_file_name(format!("{file_name}{ext}"));
            if module_path.is_file() {
                return Some(Conflict::OnComponent {
                    module_name: real_basename(&module_path),
                });
            }
        }
        return None;
    }

    // Module side: `<base>.svelte.<ext>` collides with `<base>.svelte`.
    // Stripping one extension must leave a name ending in `.svelte`, so
    // declaration files like `Foo.svelte.d.ts` can never match.
    let ext = MODULE_EXTENSIONS.iter().find(|ext| file_name.ends_with(**ext))?;
    let svelte_name = &file_name[..file_name.len() - ext.len()];
    if !svelte_name.ends_with(".svelte") {
        return None;
    }
    let svelte_path = path.with_file_name(svelte_name);
    if svelte_path.is_file() {
        return Some(Conflict::OnModule {
            svelte_name: real_basename(&svelte_path),
        });
    }
    None
}

/// Resolve the real on-disk casing of a path's basename. On
/// case-insensitive filesystems (macOS, Windows) `is_file` matches a
/// differently-cased sibling, so the constructed name may not exist with
/// that exact casing; read the directory and use the actual entry name.
fn real_basename(path: &Path) -> String {
    let wanted = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let names: Vec<String> = path
        .parent()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names
        .iter()
        .find(|name| name.as_str() == wanted)
        .or_else(|| names.iter().find(|name| name.eq_ignore_ascii_case(wanted)))
        .cloned()
        .unwrap_or_else(|| wanted.to_string())
}

pub struct NoConflictingModuleNames;

impl Rule for NoConflictingModuleNames {
    fn name(&self) -> &'static str {
        "svelte/no-conflicting-module-names"
    }

    /// Upstream: not in any preset (eslint-plugin-svelte 3.22.0+).
    fn is_recommended(&self) -> bool {
        false
    }

    /// Both sides of the collision are lintable: the component and the
    /// `.svelte.js`/`.svelte.ts`-style module.
    fn applies_to_svelte_scripts(&self) -> bool {
        true
    }

    fn run<'a>(&self, ctx: &mut LintContext<'a>) {
        // Virtual/untitled inputs (editors, in-memory tests) have no
        // siblings to collide with.
        let Some(file_path) = &ctx.file_path else {
            return;
        };
        let path = Path::new(file_path);
        match sibling_conflict(path) {
            Some(Conflict::OnComponent { module_name }) => {
                let svelte_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                ctx.diagnostic(
                    format!(
                        "The module `{module_name}` has the same name as this component. \
                         TypeScript resolves the import `./{svelte_name}` to that module, \
                         not to this component. Rename `{module_name}`."
                    ),
                    Span::new(0, 0),
                );
            }
            Some(Conflict::OnModule { svelte_name }) => {
                ctx.diagnostic(
                    format!(
                        "This module has the same name as the component `{svelte_name}`. \
                         TypeScript resolves the import `./{svelte_name}` to this module, \
                         not to the component. Rename this file."
                    ),
                    Span::new(0, 0),
                );
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CASE_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// Materialize a temp directory holding `files` and return its path.
    /// Caller passes the returned guard to `cleanup` when done.
    fn temp_case(files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxvelte-ncmn-{}-{}",
            std::process::id(),
            CASE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), "").unwrap();
        }
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reports_component_side_for_every_extension() {
        for ext in MODULE_EXTENSIONS {
            let dir = temp_case(&["Foo.svelte", &format!("Foo.svelte{ext}")]);
            let conflict = sibling_conflict(&dir.join("Foo.svelte"));
            assert_eq!(
                conflict,
                Some(Conflict::OnComponent {
                    module_name: format!("Foo.svelte{ext}")
                }),
                "extension {ext}"
            );
            cleanup(&dir);
        }
    }

    #[test]
    fn reports_module_side_for_every_extension() {
        for ext in MODULE_EXTENSIONS {
            let dir = temp_case(&["Foo.svelte", &format!("Foo.svelte{ext}")]);
            let conflict = sibling_conflict(&dir.join(format!("Foo.svelte{ext}")));
            assert_eq!(
                conflict,
                Some(Conflict::OnModule {
                    svelte_name: "Foo.svelte".to_string()
                }),
                "extension {ext}"
            );
            cleanup(&dir);
        }
    }

    #[test]
    fn ignores_declaration_files_and_different_names() {
        // `.d.ts` / `.d.svelte.ts` declaration siblings are legitimate.
        let dir = temp_case(&["Foo.svelte", "Foo.svelte.d.ts"]);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte")), None);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte.d.ts")), None);
        cleanup(&dir);

        let dir = temp_case(&["Bar.svelte", "Bar.d.svelte.ts"]);
        assert_eq!(sibling_conflict(&dir.join("Bar.svelte")), None);
        assert_eq!(sibling_conflict(&dir.join("Bar.d.svelte.ts")), None);
        cleanup(&dir);

        // Differently-named module: no collision.
        let dir = temp_case(&["Foo.svelte", "foo-state.svelte.ts"]);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte")), None);
        assert_eq!(sibling_conflict(&dir.join("foo-state.svelte.ts")), None);
        cleanup(&dir);

        // Plain `Foo.ts` is not a runes module and never collides.
        let dir = temp_case(&["Foo.svelte", "Foo.ts"]);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte")), None);
        cleanup(&dir);
    }

    #[test]
    fn ignores_module_only_pairs_and_missing_files() {
        // Two modules without a component do not trip the rule.
        let dir = temp_case(&["Foo.svelte.ts", "Foo.svelte.js"]);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte.ts")), None);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte.js")), None);
        cleanup(&dir);

        // Module whose component does not exist on disk.
        let dir = temp_case(&["Foo.svelte.ts"]);
        assert_eq!(sibling_conflict(&dir.join("Foo.svelte.ts")), None);
        cleanup(&dir);

        // Linted path that does not exist at all (virtual input).
        let dir = temp_case(&[]);
        assert_eq!(sibling_conflict(&dir.join("Ghost.svelte")), None);
        cleanup(&dir);
    }

    #[test]
    fn lints_both_sides_with_verbatim_messages() {
        let dir = temp_case(&["Foo.svelte", "Foo.svelte.ts"]);
        let component = dir.join("Foo.svelte");
        let module = dir.join("Foo.svelte.ts");

        let source = "<p>hi</p>";
        let alloc = oxc::allocator::Allocator::default();
        let parsed = crate::parser::parse_for_lint(source, &alloc);
        let diags: Vec<_> = crate::linter::Linter::all()
            .lint_with_config_and_path(
                &parsed.ast,
                source,
                crate::linter::RuleConfig::default(),
                component.to_str().unwrap(),
            )
            .into_iter()
            .filter(|d| d.rule_name == "svelte/no-conflicting-module-names")
            .collect();
        assert_eq!(diags.len(), 1);
        assert_eq!(
            diags[0].message,
            "The module `Foo.svelte.ts` has the same name as this component. \
             TypeScript resolves the import `./Foo.svelte` to that module, \
             not to this component. Rename `Foo.svelte.ts`."
        );
        assert_eq!((diags[0].span.start, diags[0].span.end), (0, 0));

        let module_diags: Vec<_> = crate::linter::Linter::all()
            .lint_svelte_script_with_config_and_path(
                "export const value = 1;",
                true,
                crate::linter::RuleConfig::default(),
                module.to_str().unwrap(),
            )
            .into_iter()
            .filter(|d| d.rule_name == "svelte/no-conflicting-module-names")
            .collect();
        assert_eq!(module_diags.len(), 1);
        assert_eq!(
            module_diags[0].message,
            "This module has the same name as the component `Foo.svelte`. \
             TypeScript resolves the import `./Foo.svelte` to this module, \
             not to the component. Rename this file."
        );

        cleanup(&dir);
    }

    #[test]
    fn skips_virtual_inputs_without_a_file_path() {
        let source = "<p>hi</p>";
        let alloc = oxc::allocator::Allocator::default();
        let parsed = crate::parser::parse_for_lint(source, &alloc);
        let diags: Vec<_> = crate::linter::Linter::all()
            .lint_with_config(&parsed.ast, source, crate::linter::RuleConfig::default())
            .into_iter()
            .filter(|d| d.rule_name == "svelte/no-conflicting-module-names")
            .collect();
        assert!(diags.is_empty());
    }
}
