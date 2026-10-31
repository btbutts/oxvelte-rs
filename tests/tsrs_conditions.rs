//! Dual-backend coverage for `@typescript-eslint/no-unnecessary-condition`:
//! every case runs through the JS sidecar AND through tsrs, with identical
//! expectations. The tsrs variant skips when no tsrs binary is on PATH.
//!
//! Fixtures live in `fixtures/tsrs/`: a Svelte 5 + TypeScript 6 project whose
//! tsconfig uses modern targets (tsrs rejects pre-TS7 options, so the
//! ES5-era upstream fixtures cannot serve this purpose).
//! `node_modules/typescript/package.json` is a version-only marker that
//! eligibility detection reads; the JS sidecar's real TypeScript comes from
//! `OXVELTE_COMPILER_RUNTIME`.

use oxvelte::config::OxvelteConfig;
use oxvelte::linter::Linter;
use oxvelte::parser;
use serde_json::json;
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

struct Case {
    name: &'static str,
    source: &'static str,
    /// Sorted expected messages for this case (identical on both backends).
    expected: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        name: "always-truthy-if",
        source: "<script lang=\"ts\">const value = true;</script>{#if value}<p>x</p>{/if}",
        expected: &[TRUTHY],
    },
    Case {
        name: "always-falsy-if",
        source: "<script lang=\"ts\">const value = false;</script>{#if value}<p>x</p>{/if}",
        expected: &[FALSY],
    },
    Case {
        name: "never-nullish-coalescing",
        source: "<script lang=\"ts\">const value = \"x\" as const;</script><p>{value ?? \"y\"}</p>",
        expected: &[NEVER_NULLISH],
    },
    Case {
        name: "always-nullish-coalescing",
        source: "<script lang=\"ts\">const value = null;</script><p>{value ?? \"y\"}</p>",
        expected: &[ALWAYS_NULLISH],
    },
    Case {
        name: "unnecessary-optional-chain",
        source: "<script lang=\"ts\">const obj = { a: 1 }; const v = obj?.a;</script><p>{v}</p>",
        expected: &[NEVER_OPTIONAL],
    },
    Case {
        name: "always-truthy-callback",
        source: "<script lang=\"ts\">const cb = (): true => true; const arr = [1]; const r = arr.filter(cb);</script><p>{r}</p>",
        expected: &[TRUTHY_FUNC],
    },
    Case {
        name: "always-falsy-callback",
        source: "<script lang=\"ts\">const cb = (): false => false; const arr = [1]; const r = arr.filter(cb);</script><p>{r}</p>",
        expected: &[FALSY_FUNC],
    },
    Case {
        name: "literal-comparison",
        source: "<script lang=\"ts\">const a = 1; const b = 2; const c = a < b;</script><p>{c}</p>",
        expected: &[LITERAL_COMPARISON],
    },
    Case {
        name: "no-overlap-comparison",
        // `let` widens b to number, so only the null side is a literal type.
        source: "<script lang=\"ts\">const a = null; let b = 1; const c = a === b;</script><p>{c}</p>",
        expected: &[NO_OVERLAP],
    },
    Case {
        name: "necessary-conditions-stay-silent",
        source: concat!(
            "<script lang=\"ts\">",
            "const maybe: string | null = Math.random() > 0.5 ? \"x\" : null;",
            "</script>",
            "{#if maybe}<p>{maybe ?? \"fallback\"}</p>{/if}"
        ),
        expected: &[],
    },
    Case {
        name: "mutable-root-template-exclusion",
        source: concat!(
            "<script lang=\"ts\">let value: true = true;</script>",
            "{#if value}<p>x</p>{/if}"
        ),
        expected: &[],
    },
];

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tsrs")
}

fn lint(source: &str, use_tsrs: bool) -> Vec<(String, u32, u32)> {
    // The JS sidecar loads TypeScript from the compiler runtime when the
    // fixture project has no real install of its own.
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        if std::env::var_os("OXVELTE_COMPILER_RUNTIME").is_none() {
            std::env::set_var(
                "OXVELTE_COMPILER_RUNTIME",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/compiler-runtime"),
            );
        }
    });
    let root = fixtures_root();
    let tsconfig = root.join("tsconfig.json");
    let settings = json!({
        "tsrs": use_tsrs,
        "typescript": { "project": tsconfig.to_string_lossy() },
    });
    let config =
        OxvelteConfig::parse(&json!({ "rules": { RULE: "error" }, "settings": settings }).to_string()).expect("config");
    let allocator = oxc::allocator::Allocator::default();
    let parsed = parser::parse_for_lint(source, &allocator);
    let path = root.join(format!("{use_tsrs}.svelte")).to_string_lossy().into_owned();
    let diagnostics: Vec<(String, u32, u32)> = Linter::all()
        .lint_with_project_config_and_path(&parsed.ast, source, &config, &path)
        .into_iter()
        .filter(|diagnostic| diagnostic.rule_name == RULE)
        .map(|diagnostic| (diagnostic.message, diagnostic.span.start, diagnostic.span.end))
        .collect();
    diagnostics
}

fn sorted(mut messages: Vec<String>) -> Vec<String> {
    messages.sort();
    messages
}

fn check_cases(use_tsrs: bool) {
    for case in CASES {
        let diagnostics = lint(case.source, use_tsrs);
        let messages = sorted(diagnostics.iter().map(|(message, _, _)| message.clone()).collect());
        let expected = sorted(case.expected.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            messages, expected,
            "case `{}` (tsrs: {use_tsrs}) messages diverge: {diagnostics:?}",
            case.name
        );
    }
}

#[test]
fn cases_via_js_sidecar() {
    check_cases(false);
}

#[cfg(feature = "tsrs-inference")]
#[test]
fn cases_via_tsrs() {
    if std::process::Command::new("tsrs").arg("--version").output().is_err() {
        eprintln!("skipping: no tsrs binary on PATH");
        return;
    }
    check_cases(true);
}

/// The strongest guarantee: both backends must agree byte-for-byte on every
/// case, including spans — not just on the expected message list.
#[cfg(feature = "tsrs-inference")]
#[test]
fn backends_agree_on_diagnostics_and_spans() {
    if std::process::Command::new("tsrs").arg("--version").output().is_err() {
        eprintln!("skipping: no tsrs binary on PATH");
        return;
    }
    for case in CASES {
        let js = lint(case.source, false);
        let tsrs = lint(case.source, true);
        assert_eq!(js, tsrs, "case `{}`: JS sidecar and tsrs diverge", case.name);
    }
}
