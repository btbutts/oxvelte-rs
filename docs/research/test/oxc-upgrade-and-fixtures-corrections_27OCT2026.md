# Plan: oxc upgrade + fixture corrections (27OCT2026)

Status: landed 27OCT2026 — all four workstreams complete; full suite and
version matrix green. This document remains as the record of the work.

## Background

Stage 2 left every suite green except the version matrix, which runs
`--strict --no-baseline` (zero tolerance — no baseline file) and therefore
fails on the eight gaps the primary parity suite records in its baseline:

- 4× `no-immutable-reactive-statements/global-return` — new
  eslint-plugin-svelte 3.23.1 cases needing `parserOptions.ecmaFeatures.
  globalReturn` support (Workstream D).
- 2× `no-unused-svelte-ignore` local cases that depend on a second rule's
  diagnostics and cannot pass under isolated evaluation (Workstream B —
  matrix only; they remain in the primary suite).
- 2× local cases whose recorded expectations predate improvements to our
  rules (Workstream C).

## Workstream A: oxc upgrade (first)

`oxc`/`oxc_diagnostics` are pinned at `=0.128.0`; 0.156.0 is current. oxc
ships constantly, so staying pinned accumulates parser bugs we work around;
upgrading more frequently keeps each patch small. This lands first because
everything else builds on the parser.

Research to do during implementation:

- **API drift between 0.128 and 0.156.** Our touchpoints are concentrated:
  `oxc::allocator::Allocator`, `oxc::parser::Parser`/`ParseOptions`,
  `oxc::semantic::SemanticBuilder`, `oxc::span::{Span, SourceType}`, the
  `oxc::ast::ast::*` node types used across `src/linter/rules/*`, and
  `oxc_diagnostics` error types in the diagnostic paths. Bump the version,
  compile, and catalog every break from the compiler errors.
- **AST shape changes.** oxc occasionally renames nodes/fields; any rename
  ripples into rule code that matches on node types.
- **Behavior changes.** Newly-correct parse errors or changed spans can alter
  rule diagnostics. The manifest-driven fixture suite is the safety net: any
  drift shows up as a named failing case.

Steps:

1. Bump `oxc`/`oxc_diagnostics` in `Cargo.toml` to `=0.156.0` (keep exact-pin
   convention), `cargo update -p oxc -p oxc_diagnostics`.
2. Fix compile errors until `cargo build` is clean.
3. `cargo test --lib`, `cargo test --test upstream_parity`,
   `cargo test --test upstream_adapter`, `cargo test --test jobsys_regressions`.
4. `cargo build --release --features custom-rules` — confirm the boa path
   still compiles against the new oxc.
5. Note in the commit message any diagnostic/span changes that required rule
   adjustments (there should be none for `upstream/` fixture expectations —
   if a rule's *output* changed, that is a behavior change to review
   explicitly, not paper over).

Relationship to `docs/research/parser/parser-parity.md`: independent layer.
That doc tracks matching svelte's own parser output; this is the JS/TS
expression parser underneath. Do not run `cargo get-svelte-fixtures` as part
of this workstream.

## Workstream B: matrix skip list for the two cross-rule local cases

Decision (approved 27OCT2026): **exclude exactly two named cases from the
version matrix**, via a short, commented skip list in
`scripts/version-matrix/run.mjs`:

- `no-unused-svelte-ignore/invalid/local-rule-unused-input.svelte`
- `no-unused-svelte-ignore/valid/local-rule-used-input.svelte`

Both live under `fixtures/linter/local/no-unused-svelte-ignore/` and depend
on a second rule (`no-at-html-tags`) being active — the ignore comment's
"used" status is defined by another rule's diagnostics, which isolated
single-rule evaluation cannot reproduce. They pass in `cargo test --lib`
(all rules active) and continue to run in the primary parity suite, where
they are recorded in `tests/upstream-parity-baseline.json`.

Scope discipline: these are the ONLY case exclusions. Nothing is excluded
from `tests/upstream_parity.rs`, and no other `local/` case is skipped
anywhere. The skip list entry must carry a comment stating the cross-rule
reason.

Implementation: in `prepareProfile`, when copying the corpus per profile,
drop the two manifest cases by id and skip writing their files.

## Workstream C: refresh two stale local expectations

Our own fixture files, updated to match our rules' current (better)
behavior, with the diffs reviewed before committing:

- `fixtures/linter/local/no-add-event-listener/invalid/module-script-errors.yaml`
  — record the fix suggestion the rule now offers ("Use `on` from
  `svelte/events` instead" with corrected output).
- `fixtures/linter/local/shorthand-attribute/invalid/comment-expression/test01-output.svelte`
  — record the comment-preserving fix output (`{disabled /* keep */}` is now
  kept, not deleted).

Then re-run `cargo get-eslint-fixtures` (rebuilds `manifest.json` from the
edited files) and regenerate the parity baseline.

## Test checkpoint (after A–C)

- `cargo test --lib`, `cargo test --test upstream_parity`,
  `cargo test --test upstream_adapter`, `cargo test --test jobsys_regressions`
  — all green.
- Version matrix, all four profiles: still red on exactly the four
  `global-return` cases (Workstream D), nothing else.

## Workstream D: `globalReturn` plumbing (last)

Goal: the four `global-return` cases run for real in the lib suite, the
primary parity suite, and every matrix profile.

What we know:

- The case configs carry `languageOptions.parserOptions.ecmaFeatures.
  globalReturn: true` (visible in `fixtures/linter/manifest.json`).
- `tests/upstream_parity.rs::rule_config` currently flags any `parserOptions`
  key other than `svelteConfig` (and `project` for the type-aware rule) as a
  `parser_configuration` gap — that is where the four cases land today.
- Script parsing happens in `src/linter/mod.rs::lint_impl` (two
  `Parser::new(&alloc, &s.content, st).parse()` sites, instance and module)
  before per-rule configs are applied; `RuleConfigSource::Fixed(config)` is
  already in scope there, so the flag can be read from the case config without
  restructuring the driver.
- oxc: `Parser::new(...).with_options(ParseOptions {
  allow_return_outside_function: true, ..Default::default() })` (present
  since well before 0.128; re-verify the exact options API after the 0.156
  upgrade).

Implementation sketch:

1. Decide the internal carrier: read `ecmaFeatures.globalReturn` from the
   config inside `lint_impl` (both `Fixed` and `Project` variants — project
   configs may legitimately set it too) and apply
   `allow_return_outside_function` at both parse sites. Also cover plain
   `.js`/`.ts` inputs (`lint_script_with_config_and_path` /
   `lint_svelte_script_with_config_and_path`) for completeness.
2. In `tests/upstream_parity.rs::rule_config`, stop flagging
   `ecmaFeatures.globalReturn` as an unsupported parser configuration — it is
   now supported. (The `parser_configuration` issue category remains for
   genuinely unsupported keys.)
3. Lib harness: nothing extra — `config_for_case` passes case config through;
   verify the four cases now exercise the flag.
4. Validate: the four cases pass in `cargo test --lib`,
   `cargo test --test upstream_parity` (regenerate the baseline so their
   entries drop out), and the full version matrix in strict mode.

## Done means

- `oxc`/`oxc_diagnostics` at 0.156.x, all suites green.
- The two cross-rule local cases skipped by the matrix only, with the reason
  documented in the skip list; everything else running everywhere.
- Zero `parser_configuration` gaps from the `global-return` cases.
- Version matrix green in all four profiles with `--strict --no-baseline`.
