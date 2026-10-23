# Close the rust - js (boa) conformity gap

### Preface:
The boa team has released v0.22 (28AUG2026), achieving a conformity of 95.60% in the official ECMAScript Test Suite (Test262), up from 89.92% in our current version v0.20 (05DEC2024). At only about 90% conformance from the current engine, we are certainly missing modern language features, syntax constructs and scoping behaviors vanilla js will support. Furthermore, at nearly 2 years old, the older version is certainly out of support.

Whilst this project does have a defined custom_rules syntax which the current boa engine v0.20 parses, the nature of this implementation is that we are running untested, untrusted code from the oxvelte app perspective, thus ensuring maximal conformity of ECMAScript-262 is vital to ensure users can pass custom rules written in js using a syntax they expect. Likewise, under our own development, we want to ensure we parse the known standard accurately, and do no rely on older broken quirks in an experimental runtime that are likely already out of support.

Upgrading boa 0.20 to 0.22 is a breaking change, thus this document serves as the primary research document to notate and plan the adaptation of our boa wrapper to support it.

### Current usage audit (boa v0.20)

Our entire boa surface area lives in `src/custom_rules.rs`, gated behind `--features custom-rules` (`Cargo.toml:22`, `src/lib.rs:8-9`). No other file under `src/` imports a `boa_*` crate; `src/main.rs:223-230`, `src/config.rs`, and `src/linter/mod.rs` only reference the feature flag and the `Rule` trait abstraction, so they are not expected to need changes.

The exact boa APIs we consume:

| API | Location | Purpose |
|---|---|---|
| `boa_engine::{Context, Source}` import | `src/custom_rules.rs:29` | engine entry points |
| `Context::default()` | `src/custom_rules.rs:73, 248` | one engine per rule run / meta extraction |
| `Context::eval(Source::from_bytes(...))` | `src/custom_rules.rs:136, 251, 263` | eval rule prelude + rule source |
| `JsValue::to_string(&mut Context)` | `src/custom_rules.rs:140, 254, 268` | stringify result JSON |
| `JsString::to_std_string_escaped()` | `src/custom_rules.rs:142, 256, 269` | convert to Rust `String` |
| `format!("{}", JsError)` | `src/custom_rules.rs:137, 252, 266` | error reporting |

### Findings from the v0.21 / v0.22 changelogs

Sources: [boa v0.21 release blog](https://boajs.dev/blog/2025/10/22/boa-release-21), [v0.22 release notes](https://github.com/boa-dev/boa/releases/tag/v0.22), [boa CHANGELOG](https://github.com/boa-dev/boa/blob/main/CHANGELOG.md), docs.rs pages for `boa_engine` 0.22.0.

**API compatibility — no code changes expected.** Every API in the table above exists unchanged in 0.22.0; the official docs.rs front-page example for 0.22.0 is byte-for-byte our call pattern (`Context::default()` → `eval(Source::from_bytes(...))` → `to_string(...)` → `to_std_string_escaped()`). The headline v0.21 breaking changes (JobQueue → JobExecutor revamp, async ModuleLoader, register VM, NaN-boxed `JsValue`) only affect embedders who customize the job queue, load native ES modules, or touch `JsValue` internals — we do none of these.

**Behavioral change 1 — error strings gain backtraces (v0.21, PR #4292).** `JsError`'s `Display` now appends source-position info and `\n    at <location>` backtrace lines. Our `.map_err(|e| format!("{}", e))` sites will produce longer, multi-line messages. This affects anything matching on error text: the `"parse error: {}"` / `"error in custom rule {}"` stderr output, and any snapshot or exact-match tests. This is a net positive (rule authors get locations) but must be accounted for in tests.

**Behavioral change 2 — stricter, more conformant parsing.** Moving from 89.92% → 95.60% Test262 conformance means syntax/behavior that v0.20 accepted incorrectly may now (correctly) throw, and previously-throwing valid syntax now works. Existing user rule files should be re-tested; our own fixtures under `examples/custom-rules/` and `fixtures/` should be exercised against the new engine. Rule authors also gain new builtins: Set methods, Iterator helpers, `RegExp.escape`, `Promise.allKeyed`, `Error.isError`, `Math.sumPrecise`, and Temporal (enabled by default in v0.22).

**Behavioral change 3 — NaN-boxed `JsValue` (v0.21).** Now the default representation; boa notes it is not compatible with all platforms, with a `jsvalue-enum` fallback feature. Fine on our macOS/Linux targets, but worth noting if a platform build ever misbehaves.

### Areas to modify

1. **`Cargo.toml:36`** — bump `boa_engine = { version = "0.20", optional = true }` to `version = "0.22"`, keeping boa's default features enabled (`default-features = true`, i.e. no change to how we declare the dependency today). Note: v0.20 had no default features, but 0.21 added `float16` + `xsum` and 0.22 adds `temporal` (pulling in `temporal_rs`, `icu_calendar`, `timezone_provider`), so keeping defaults does grow the dependency tree — but it also keeps the full builtin surface (`Temporal`, `Float16Array`, `Math.sumPrecise`) available to rule authors, and keeps our declaration idiomatic. **Decision: ship with defaults on.** After the upgrade lands, inspect the compiled binary size; if the growth is unacceptable we can revisit with `default-features = false` as a follow-up.
2. **`Cargo.lock`** — all eight `boa_*` crates (`boa_ast`, `boa_engine`, `boa_gc`, `boa_interner`, `boa_macros`, `boa_parser`, `boa_profiler`, `boa_string`) move 0.20.0 → 0.22.0 together. Note: the `profiler` feature was removed in v0.21, so `boa_profiler` may drop out of the tree; new transitive deps from the default features (`temporal_rs`, `icu_calendar`, `timezone_provider`, etc.) will appear.
3. **`src/custom_rules.rs`** — no call-site changes anticipated per the audit above. Re-verify compilation and review error-path formatting at lines 137, 252, 266 given the new backtrace output; decide whether multi-line boa errors should be trimmed or passed through verbatim.
4. **Tests / fixtures** — the unit tests in `src/custom_rules.rs:303-321` plus any snapshot tests touching custom-rule error output (`fixtures/`, `tests/upstream_parity.rs`, insta snapshots) must be re-run with `--features custom-rules`; update any that match on boa error text.
5. **Toolchain** — boa 0.22 requires Rust ≥ 1.91 (edition 2024; MSRV jumped 1.82 → 1.88 → 1.91 across 0.20 → 0.22). `rust-toolchain.toml` pins `stable`, so local builds are fine, but CI and the release pipeline (`scripts/build-npm.sh`, `scripts/release.sh`, `.github/`) must be confirmed to run a sufficiently recent stable.

### Validation plan

1. `cargo build --release --features custom-rules` — engine compiles and links.
2. `cargo build --release` (no feature) — confirm the default binary is untouched.
3. `cargo test --features custom-rules` — unit tests in `custom_rules.rs` pass.
4. Run the linter against `examples/custom-rules/` fixtures with both a passing and a failing rule, confirming diagnostics, fixes, and the new backtrace-bearing error output look sane.
5. Re-run parity/snapshot suites that involve custom rules and bless any insta snapshot diffs caused by error-format changes.
6. Compare the release binary size of the `custom-rules` build before and after the upgrade; if the default features (`float16`, `xsum`, `temporal`) grew it unacceptably, evaluate `default-features = false` as a follow-up.

### Validation results (v0.22.0, `default-features = true`, branch `programatic-deps-updates`, 22OCT2026)

Upgrade performed via `cargo add boa_engine@0.22.0` (kept `optional = true`; `Cargo.lock` regenerated by cargo). No code changes were needed anywhere — `src/custom_rules.rs` compiles and runs unmodified, confirming the audit.

1. **Builds** — `cargo build --release --features custom-rules` and `cargo build --release` (no feature) both succeed after `cargo clean`.
2. **Tests** — `cargo test --features custom-rules`: 625 lib + 44 integration tests, 0 failures; the `upstream_parity` harness reports "No new gaps relative to the baseline." No insta snapshots exist in the repo, so nothing to bless.
3. **Fixture runs** — exercised against a scratch project using `examples/custom-rules/no-div-without-class.js`:
   - ESM (`export default`) and CommonJS (`module.exports`) rule forms both load and report correctly.
   - `ctx.diagnostic` and `ctx.diagnosticWithFix` both work; `--fix` applies replacements at the correct spans (a rule that reports unconditionally re-fires on each fix pass, matching ESLint's 10-pass cap semantics — expected, rule-author concern only).
   - **Backtrace change confirmed**: a rule throwing at runtime now prints e.g. `TypeError: cannot convert 'null' or 'undefined' to object (unknown at :51:11)` followed by `    at run (...)` / `    at <main> (...)` frames. Output is multi-line but readable; passed through verbatim, no trimming needed.
   - **New builtins verified working inside rules**: `Set.prototype.union`, Iterator helpers (`.values().map().reduce()`), `Math.sumPrecise` (xsum), `Temporal.PlainDate` (temporal). Note: `RegExp.escape("a.b")` returns `\x61\.b` — this is the spec-correct encoding (leading ASCII letters are hex-escaped), a good demonstration of the conformance jump.
4. **Binary size** — `custom-rules` release binary: **9.9 MB (boa 0.20) → 12 MB (boa 0.22, defaults on)**, ~+2 MB. No-feature binary remains ~5 MB. The `docs/custom-rules.md` claim that the engine "roughly doubles the binary size" still holds. Next: repeat this validation with `default-features = false` to measure what `float16`/`xsum`/`temporal` cost, then decide the final feature set.

### Validation results (v0.22.0, `default-features = false`, 22OCT2026)

Same procedure repeated with `default-features = false` and no features manually re-enabled in `Cargo.toml`.

1. **Builds & tests** — `cargo build --release --features custom-rules`, `cargo test --lib`, and `cargo test --features custom-rules` all succeed (same 625 lib + 44 integration tests, 0 failures).
2. **Feature probe** — a probe rule confirmed exactly which builtins the default features gate:
   - Still available (core engine, not feature-gated): `Set.prototype.union`, Iterator helpers, `RegExp.escape`.
   - Removed with `default-features = false`: `Float16Array` (`float16`), `Math.sumPrecise` (`xsum`), `Temporal.*` (`temporal`). A rule touching one of these fails at runtime with `TypeError: not a callable function` (or `ReferenceError`), surfaced via our normal `error in custom rule` path with the new backtrace.
3. **Diagnostics & fixes** — `ctx.diagnostic`, `ctx.diagnosticWithFix`, and `--fix` behave identically to the defaults-on build (10-pass fix cap confirmed).
4. **Binary size** — **11 MB** with defaults off vs **12 MB** defaults on vs **9.9 MB** on boa 0.20. Disabling defaults saves ~1 MB; the remaining ~1.1 MB growth over 0.20 is the engine itself.

**Final decision (22OCT2026): ship with `default-features = true`.** The ~1 MB saved by disabling defaults was judged insignificant against the cost of rule authors losing `Temporal`, `Float16Array`, and `Math.sumPrecise`. `Cargo.toml` declares `boa_engine = { version = "0.22.0", optional = true }` - with `default-features = false` now removed, `= true` is implicit, so that option is removed from `Cargo.toml`. Final state re-verified after the flip back: release build 12 MB, all 669 tests pass, and the feature probe confirms all six builtins (`Set.union`, Iterator helpers, `RegExp.escape`, `Float16Array`, `Math.sumPrecise`, `Temporal`) available inside rules.