# Upstream rule parity

Install the pinned compiler runtime before running the fixture suite:

```sh
npm ci --prefix scripts/compiler-runtime --ignore-scripts --no-audit --no-fund
```

Set `OXVELTE_COMPILER_RUNTIME` to that directory. In PowerShell:

```powershell
$env:OXVELTE_COMPILER_RUNTIME = (Resolve-Path scripts/compiler-runtime).Path
```

On Linux or macOS:

```sh
export OXVELTE_COMPILER_RUNTIME="$(pwd)/scripts/compiler-runtime"
```

Then run the pinned eslint-plugin-svelte fixture suite:

```sh
cargo test --locked --test upstream_parity -- --report reports/upstream-parity.json
```

The runner checks exact diagnostic counts, messages and one-based UTF-16 line/column positions. It preserves duplicate findings and tests fix output in one pass, matching the upstream fixture generator. Each case runs only its target rule. Our own regression fixtures live in `fixtures/linter/local/`; the byte-identical upstream mirror lives in `fixtures/linter/upstream/`, and both are indexed by `fixtures/linter/manifest.json`.

The corpus is pinned to the [eslint-plugin-svelte 3.23.1 release](https://github.com/sveltejs/eslint-plugin-svelte/releases/tag/eslint-plugin-svelte%403.23.1) by the `OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG` variable in `.cargo/config.toml`. Every corpus file has a SHA-256 hash recorded in the manifest. `.gitattributes` preserves upstream bytes on Windows. Missing, changed and extra corpus files fail the check.

The corpus contains 1,385 cases across 84 rules. 83 rules use the upstream fixture-loader naming convention; `no-conflicting-module-names` uses custom tests with ordinary `Foo.svelte` filenames instead of that loader, so the importer synthesizes its 23 cases (expectations mirroring upstream's RuleTester source) and its rule is implemented in Rust. The declared comparison environment uses JobSys's current locked Svelte 5.49.2, ESLint 10.9.1, TypeScript 6.0.3 and @typescript-eslint/parser 8.70.0. These versions control dependency eligibility; the Rust runner does not invoke ESLint. Compiler-backed cases invoke Svelte, and type-aware cases invoke the TypeScript checker through the embedded JavaScript runtime (bun preferred, node fallback — see "Reproducing the import").

## Initial results

| Result | Cases |
| --- | ---: |
| All checked expectations match | 689 |
| Mismatches or unsupported capabilities | 546 |
| Outside the declared dependency versions | 64 |

| Priority rule | Match | Gap | Version skip |
| --- | ---: | ---: | ---: |
| require-each-key | 3 | 0 | 0 |
| no-navigation-without-resolve | 36 | 48 | 0 |
| no-unused-props | 56 | 20 | 0 |
| prefer-svelte-reactivity | 78 | 1 | 0 |
| prefer-writable-derived | 2 | 7 | 0 |
| valid-prop-names-in-kit-pages | 7 | 4 | 11 |

These are fixture results, not real-project finding counts. A gap can be a location or message mismatch even when both tools report the same number of findings. All seven writable-derived invalid cases match their diagnostic expectations but require suggestions, which Oxvelte does not expose. The initial reactivity gap was a default-exported binding in a `.svelte.js` module; it is now fixed and all 79 reactivity cases match. Navigation cases expose argument-location differences as well as semantic mismatches. All 56 unused-props valid cases pass. Its 20 invalid cases have diagnostic differences, mostly report locations, with one custom-options case missing a finding.

Unused props now matches all 76 cases, navigation resolve all 84, and writable derived all nine. The [seventh parallel wave report](research/upstream-parity-seventh-wave.md) records the latest changes: all seven type-aware condition cases match. After the fixture consolidation (the 3.23.1 refresh and the merge of our local fixtures into the manifest), the suite stands at 1,293 matches, 2 known gaps and 67 version skips; the two gaps are our own cross-rule `no-unused-svelte-ignore` cases, which cannot run under isolated single-rule evaluation by design. The current full-suite counts and remaining issues are in the generated gap inventory below.

The JSON report contains every case ID, status and expected/actual difference. It separates count, diagnostic, fix, parse, version and capability issues. It records unsupported rules, executable configs, compiler-dependent rules, extra parser settings and suggestions. All rule options are passed through. Typed cases run and can fail; they are not skipped wholesale. Unsupported capabilities prevent a case being counted as a full match even when its diagnostics agree.

## Tracking progress

The [gap inventory](research/upstream-parity-gaps.md) categorizes every outstanding case. The [commit plan](research/upstream-parity-plan.md) proposes separate fixes and names the deferred capabilities. Regenerate the inventory after each fix with `node scripts/summarize-parity.mjs` using a fresh full report.

```sh
# Focus on a rule. An unknown name fails instead of running zero tests.
cargo test --locked --test upstream_parity -- --rule no-unused-props --report reports/props-parity.json

# Require every eligible case to match, ignoring the known-failure allowance.
cargo test --locked --test upstream_parity -- --rule require-each-key --strict

# Review the full report before explicitly replacing the known-gap baseline.
cargo test --locked --test upstream_parity -- --update-baseline --report reports/upstream-parity.json
```

`tests/upstream-parity-baseline.json` stores issue signatures by case and comparison dimension. CI rejects new or changed gaps and changed version skips. Removed issues are improvements and pass. Updates require a full suite run and are tied to the manifest hash, including its environment. Updating the baseline does not edit upstream expectations. CI runs the adapter tests and parity runner on Windows and Linux and uploads the full report.

The initial baseline was inspected for the priority rules and grouped by issue category. It is a record of current compatibility debt, not a claim that 546 gaps are acceptable final behavior. Fix rules against the unchanged expectations and reduce the baseline as improvements land.

## Reproducing the import

Fixtures are imported by the cargo-driven, tag-pinned pipeline described in [the pinned-rules plan](research/test/pinned-rules.md). The pinned release tags live in `.cargo/config.toml`; bumping a version and re-running the matching command is the entire refresh process:

```sh
cargo get-eslint-fixtures   # fixtures/linter/upstream/ + manifest.json from eslint-plugin-svelte
cargo get-svelte-fixtures   # fixtures/parser/{legacy,modern}/ from sveltejs/svelte
```

Both commands run `scripts/get-fixtures/import.mjs` through a thin Rust launcher that prefers `bun` and falls back to `node` (override with `OXVELTE_JS_RUNTIME`), installing the script's dependencies on first run. Regular parity runs require Cargo, a JavaScript runtime and the pinned compiler runtime above; CI installs all dependency sets and sets the runtime path on Windows and Linux.

The importer downloads the pinned release's GitHub source archive (no git checkout needed), syncs rule directories into `fixtures/linter/upstream/` byte-identically (reporting `CREATE`/`UPDATE`/`SKIP` per rule and listing local rules with no upstream candidate), and rebuilds `fixtures/linter/manifest.json` over both fixture roots. It reproduces the upstream loader's filename discovery, JSON/JS/CJS config precedence and dependency filters. It exports original YAML diagnostics and fix outputs to the manifest, preserving suggestions and parser/config metadata. JS/CJS configs are retained unchanged; eligible compiler configuration modules are executed by the compiler bridge. Missing error/fix snapshots fail the import; it never generates expectations by running Oxvelte. `fixtures/linter/local/` — our own cases, same layout — is never written by the importer.

Metadata inspection uses the exact declared ESLint and TypeScript versions to resolve inherited core fixability and statically proven compiler-version guards, read from the TypeScript rule sources held only in a throwaway temp directory (they never enter the repo). Version gates are stored per case in a machine-readable `gates` field so the version matrix can recompute eligibility per profile without those sources. Diagnostic filename spelling is normalized only for the evaluated fixture when comparing messages; actual file paths remain available to project resolution.

For an upstream refresh, bump the tag in `.cargo/config.toml`, re-run `cargo get-eslint-fixtures`, review the per-rule report and the manifest diff, then regenerate the baseline (`--update-baseline`) after a full reviewed report. The importer is additive-only: rules upstream deletes stay in place and are listed as having no upstream candidate. The generated `fixtures/linter/package.json` binds Oxvelte's nearest-package Svelte-version signal to the declared environment.

## Version matrix

All 67 primary version skips now pass in matching pinned environments. The four additional profiles cover Svelte 3, Svelte 4, newer Svelte 5, ESLint 8/9 and the older TypeScript parser. Each runs against unchanged expectations with `--strict --no-baseline`; all eligible cases pass. Across the environments, all 1,362 imported cases are covered, except two local cross-rule `no-unused-svelte-ignore` cases the matrix skips by name (their expectations depend on a second rule's diagnostics, which isolated evaluation cannot reproduce). The primary baseline remains at 1,293 passes, two gaps and 67 version skips.

See the [version matrix report](research/upstream-parity-version-matrix.md) for exact versions, reproduction commands, validation and per-case coverage. CI runs each profile on Windows and Linux.

## Remaining coverage

The [JobSys check from 9 October 2026](research/jobsys-parity-2026-10-09.md) found differences beyond the passing fixture corpus. The [follow-up fixes](research/jobsys-parity-fixes-2026-10-09.md) cover unused-prop parsing and consumption, Kit 3 navigation eligibility, block-scoped function options and nested style validation. All 1,029 comparable Oxvelte diagnostics now match exactly; the only unmatched reference entry is a duplicate ESLint reactivity diagnostic.

This first runner covers the raw rule fixtures. The imported TypeScript/core integration test sources and processor/config/settings tests are available for later ports but are not executed. It also does not run a reference ESLint installation to validate each snapshot under the declared environment. Expectations come from the pinned upstream files. All eligible raw rule fixtures now match, including the synthesized `no-conflicting-module-names` cases. Type-aware condition checks have scope limitations documented in the seventh-wave report. The existing real-project parity script remains useful alongside these checks.

See the [research note](research/eslint-plugin-svelte-test-suite.md) for the upstream test architecture and the JobSys motivation. The upstream MIT license is copied into the corpus and its attribution is retained in `THIRD_PARTY_NOTICES`.

After commits 1-8, the suite has 762 matches, 473 gaps and 64 version skips. Unused props matches all 76 cases, navigation-without-resolve matches all 84, and Kit prop names matches all 11 eligible cases with 11 legacy version skips. See the [progress report](research/upstream-parity-progress.md) and [current gap inventory](research/upstream-parity-gaps.md).

The [first parallel wave](research/upstream-parity-first-wave.md) brings the suite to 789 matches, 446 gaps and 64 version skips. Suggestions are now compared as exact per-diagnostic alternatives. Duplicate directive and navigation-base fixtures all match.
