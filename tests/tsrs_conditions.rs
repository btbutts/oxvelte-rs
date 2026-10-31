//! Dual-backend coverage for `@typescript-eslint/no-unnecessary-condition`:
//! every case runs through the JS sidecar AND through tsrs with identical
//! expectations, plus a byte-for-byte agreement pass. The tsrs variants skip
//! when no tsrs binary is on PATH or the linter fixtures have not been
//! fetched (`cargo get-eslint-fixtures`).
//!
//! Coverage is two-tier:
//! - an inline corpus below covering every message kind, run against the
//!   fixtures/linter project (Svelte 5.49.2 + TypeScript 6.0.3, es6-target
//!   tsconfig — see scripts/get-fixtures/import.mjs);
//! - the upstream corpus: the rule's cases from fixtures/linter/manifest.json.

// This suite uses only a few of the shared parity helpers; the rest are dead
// code in this binary (they are exercised by the other parity suites).
#[allow(dead_code)]
#[path = "support/parity.rs"]
mod parity;

use oxvelte::config::OxvelteConfig;
use oxvelte::linter::{Linter, RuleConfig};
use oxvelte::parser;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const RULE: &str = "@typescript-eslint/no-unnecessary-condition";

const TRUTHY: &str = "Unnecessary conditional, value is always truthy.";
const FALSY: &str = "Unnecessary conditional, value is always falsy.";
const NEVER_NULLISH: &str =
    "Unnecessary conditional, expected left-hand side of `??` operator to be possibly null or undefined.";
const ALWAYS_NULLISH: &str =
    "Unnecessary conditional, left-hand side of `??` operator is always `null` or `undefined`.";
const NEVER_OPTIONAL: &str = "Unnecessary optional chain on a non-nullish value.";
const TRUTHY_FUNC: &str = "This callback should return a conditional, but return is always truthy.";
const FALSY_FUNC: &str = "This callback should return a conditional, but return is always falsy.";
const LITERAL_COMPARISON: &str = "Unnecessary conditional, both sides of the expression are literal values.";
const NO_OVERLAP: &str = "Unnecessary conditional, the types have no overlap.";

struct InlineCase {
    name: &'static str,
    source: &'static str,
    /// Sorted expected messages for this case (identical on both backends).
    expected: &'static [&'static str],
}

const INLINE_CASES: &[InlineCase] = &[
    InlineCase {
        name: "always-truthy-if",
        source: "<script lang=\"ts\">const value = true;</script>{#if value}<p>x</p>{/if}",
        expected: &[TRUTHY],
    },
    InlineCase {
        name: "always-falsy-if",
        source: "<script lang=\"ts\">const value = false;</script>{#if value}<p>x</p>{/if}",
        expected: &[FALSY],
    },
    InlineCase {
        name: "never-nullish-coalescing",
        source: "<script lang=\"ts\">const value = \"x\" as const;</script><p>{value ?? \"y\"}</p>",
        expected: &[NEVER_NULLISH],
    },
    InlineCase {
        name: "always-nullish-coalescing",
        source: "<script lang=\"ts\">const value = null;</script><p>{value ?? \"y\"}</p>",
        expected: &[ALWAYS_NULLISH],
    },
    InlineCase {
        name: "unnecessary-optional-chain",
        source: "<script lang=\"ts\">const obj = { a: 1 }; const v = obj?.a;</script><p>{v}</p>",
        expected: &[NEVER_OPTIONAL],
    },
    InlineCase {
        // An annotated return type keeps the callback's literal un-widened.
        name: "always-truthy-callback",
        source: "<script lang=\"ts\">const cb = (): true => true; const arr = [1]; const r = arr.filter(cb);</script><p>{r}</p>",
        expected: &[TRUTHY_FUNC],
    },
    InlineCase {
        name: "always-falsy-callback",
        source: "<script lang=\"ts\">const cb = (): false => false; const arr = [1]; const r = arr.filter(cb);</script><p>{r}</p>",
        expected: &[FALSY_FUNC],
    },
    InlineCase {
        name: "literal-comparison",
        source: "<script lang=\"ts\">const a = 1; const b = 2; const c = a < b;</script><p>{c}</p>",
        expected: &[LITERAL_COMPARISON],
    },
    InlineCase {
        name: "no-overlap-comparison",
        // `let` widens b to number, so only the null side is a literal type.
        source: "<script lang=\"ts\">const a = null; let b = 1; const c = a === b;</script><p>{c}</p>",
        expected: &[NO_OVERLAP],
    },
    InlineCase {
        name: "necessary-conditions-stay-silent",
        source: concat!(
            "<script lang=\"ts\">",
            "const maybe: string | null = Math.random() > 0.5 ? \"x\" : null;",
            "</script>",
            "{#if maybe}<p>{maybe ?? \"fallback\"}</p>{/if}"
        ),
        expected: &[],
    },
    InlineCase {
        name: "mutable-root-template-exclusion",
        source: concat!(
            "<script lang=\"ts\">let value: true = true;</script>",
            "{#if value}<p>x</p>{/if}"
        ),
        expected: &[],
    },
];

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    rule: String,
    filename: String,
    config: Value,
    errors: Vec<Value>,
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/linter")
}

/// The fixtures project (tsconfig + version markers) must have been fetched.
fn fixtures_available() -> bool {
    fixtures_root().join("upstream/tsconfig.json").is_file()
        && fixtures_root().join("node_modules/typescript/package.json").is_file()
}

fn rule_config(use_tsrs: bool) -> RuleConfig {
    let tsconfig = fixtures_root().join("upstream/tsconfig.json");
    RuleConfig {
        options: None,
        settings: Some(json!({
            "tsrs": use_tsrs,
            "typescript": { "project": tsconfig.to_string_lossy() },
        })),
    }
}

fn lint(source: &str, path: &str, config: &RuleConfig) -> Vec<(String, u32, u32)> {
    // The JS sidecar loads TypeScript from the compiler runtime when the
    // fixtures project has no real install of its own.
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        if std::env::var_os("OXVELTE_COMPILER_RUNTIME").is_none() {
            std::env::set_var(
                "OXVELTE_COMPILER_RUNTIME",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/compiler-runtime"),
            );
        }
    });
    let allocator = oxc::allocator::Allocator::default();
    let parsed = parser::parse_for_lint(source, &allocator);
    Linter::all()
        .lint_with_project_config_and_path(&parsed.ast, source, &config_for(config), path)
        .into_iter()
        .filter(|diagnostic| diagnostic.rule_name == RULE)
        .map(|diagnostic| (diagnostic.message, diagnostic.span.start, diagnostic.span.end))
        .collect()
}

fn config_for(rule_config: &RuleConfig) -> OxvelteConfig {
    let mut config = json!({ "rules": { RULE: "error" } });
    if let Some(settings) = &rule_config.settings {
        config["settings"] = settings.clone();
    }
    if let Some(options) = &rule_config.options {
        config["rules"][RULE] = json!(["error", options]);
    }
    OxvelteConfig::parse(&config.to_string()).expect("config")
}

fn sorted(mut messages: Vec<String>) -> Vec<String> {
    messages.sort();
    messages
}

fn check_inline_cases(use_tsrs: bool) {
    let config = rule_config(use_tsrs);
    for case in INLINE_CASES {
        let path = fixtures_root().join(format!("{}.svelte", case.name));
        let diagnostics = lint(case.source, &path.to_string_lossy(), &config);
        let messages = sorted(diagnostics.iter().map(|(message, _, _)| message.clone()).collect());
        let expected = sorted(case.expected.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            messages, expected,
            "case `{}` (tsrs: {use_tsrs}) messages diverge: {diagnostics:?}",
            case.name
        );
    }
}

/// The rule's upstream manifest cases, linted through one backend and
/// compared against the manifest's expectations (message/line/column).
fn upstream_diagnostics(use_tsrs: bool) -> Vec<(String, Vec<Value>, Vec<Value>)> {
    let root = fixtures_root();
    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    for case in manifest.cases.iter().filter(|case| case.rule == RULE) {
        let path = root.join(&case.filename);
        let source = std::fs::read_to_string(&path).unwrap();
        let config = parity::rewrite_fixture_paths(&case.config, &root, &case.filename);
        let mut settings = parity::fixture_settings(&config, None, &root, RULE);
        settings.get_or_insert_with(|| json!({}))["tsrs"] = json!(use_tsrs);
        let rule_config = RuleConfig {
            options: config.get("options").cloned(),
            settings,
        };
        let diagnostics = lint(&source, &path.to_string_lossy(), &rule_config);
        let actual: Vec<Value> = diagnostics
            .iter()
            .map(|(message, start, _)| {
                let (line, column) = parity::location(&source, *start).unwrap();
                json!({ "message": message, "line": line, "column": column })
            })
            .collect();
        let expected: Vec<Value> = case
            .errors
            .iter()
            .map(|error| {
                json!({
                    "message": error["message"],
                    "line": error["line"],
                    "column": error["column"],
                })
            })
            .collect();
        results.push((case.filename.clone(), expected, actual));
    }
    results
}

fn check_upstream_cases(use_tsrs: bool) {
    for (filename, expected, actual) in upstream_diagnostics(use_tsrs) {
        assert_eq!(actual, expected, "{filename} (tsrs: {use_tsrs}) diverges");
    }
}

#[test]
fn inline_cases_via_js_sidecar() {
    if !fixtures_available() {
        eprintln!("skipping: linter fixtures not fetched");
        return;
    }
    check_inline_cases(false);
}

#[test]
fn upstream_cases_via_js_sidecar() {
    if !fixtures_available() {
        eprintln!("skipping: linter fixtures not fetched");
        return;
    }
    check_upstream_cases(false);
}

#[cfg(feature = "tsrs-inference")]
#[test]
fn inline_cases_via_tsrs() {
    if std::process::Command::new("tsrs").arg("--version").output().is_err() || !fixtures_available() {
        eprintln!("skipping: no tsrs binary on PATH or fixtures not fetched");
        return;
    }
    check_inline_cases(true);
}

#[cfg(feature = "tsrs-inference")]
#[test]
fn upstream_cases_via_tsrs() {
    if std::process::Command::new("tsrs").arg("--version").output().is_err() || !fixtures_available() {
        eprintln!("skipping: no tsrs binary on PATH or fixtures not fetched");
        return;
    }
    check_upstream_cases(true);
}

/// The strongest guarantee: both backends must agree byte-for-byte on every
/// case, including spans — not just on the expected message list.
#[cfg(feature = "tsrs-inference")]
#[test]
fn backends_agree_on_diagnostics_and_spans() {
    if std::process::Command::new("tsrs").arg("--version").output().is_err() || !fixtures_available() {
        eprintln!("skipping: no tsrs binary on PATH or fixtures not fetched");
        return;
    }
    for case in INLINE_CASES {
        let path = fixtures_root().join(format!("{}.svelte", case.name));
        let path = path.to_string_lossy().into_owned();
        let js = lint(case.source, &path, &rule_config(false));
        let tsrs = lint(case.source, &path, &rule_config(true));
        assert_eq!(js, tsrs, "case `{}`: JS sidecar and tsrs diverge", case.name);
    }
    for (filename, js, tsrs_backend) in upstream_diagnostics(false)
        .into_iter()
        .zip(upstream_diagnostics(true))
        .map(|((filename, _, js), (_, _, tsrs))| (filename, js, tsrs))
    {
        assert_eq!(js, tsrs_backend, "{filename}: JS sidecar and tsrs diverge");
    }
}
