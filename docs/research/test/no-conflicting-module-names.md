# Plan: implement `svelte/no-conflicting-module-names`

Status: researched 28OCT2026, not yet implemented. This is the one rule that
has synced fixtures in `fixtures/linter/upstream/` (eslint-plugin-svelte
3.23.1) but no Rust implementation — the other 83 upstream fixture rules are
all covered (`indent` lives in the `indent/` module directory;
`@typescript-eslint/no-unnecessary-condition` maps to our native
`no_unnecessary_condition.rs`).

## What the rule does

Since Svelte 5, runes modules are named `Foo.svelte.ts` / `Foo.svelte.js`.
When a component `Foo.svelte` and a module `Foo.svelte.ts` sit in the same
directory, the specifier `./Foo.svelte` is ambiguous: bundlers and
svelte-check resolve it to the component, but plain TypeScript (`tsc`,
typescript-eslint) appends `.ts` and lands on the module. The rule reports
**both** files of the collision (a lint run may contain only one of them).

- Upstream rule source (3.23.1):
  `packages/eslint-plugin-svelte/src/rules/no-conflicting-module-names.ts`
- Docs: `svelte/no-conflicting-module-names`, since v3.22.0, category
  "Possible Errors", **not** in any preset, no options, not fixable.

### Matching semantics (confirmed against upstream source + fixtures)

The rule cares only about files whose extension chain contains `.svelte`
immediately after the base name:

- A **component** is any file ending in `.svelte` (`Foo.svelte`).
- A **conflicting module** is a sibling whose name is exactly
  `<same-base>.svelte.<ext>` where `<ext>` is one of
  `.ts .tsx .js .jsx .mts .cts .mjs .cjs` — i.e. one of those eight
  extensions immediately right of `.svelte`, nothing in between.
- A collision requires **both** sides in the **same directory** with the
  **same base name**: `Foo.svelte` + `Foo.svelte.ts` trip;
  `Foo.svelte` + `Foo-Too.svelte.mts` do not (different base);
  `Foo.svelte` + `Foo.svelte.d.ts` do not (the `.d` between `.svelte` and
  `.ts` breaks the chain — stripping one extension leaves `Foo.svelte.d`,
  which is not a component); likewise `Foo.svelte.a.ts`, `Foo.svelte.2.ts`
  etc. never match.
- Plain `Foo.ts` next to `Foo.svelte` never matches — the `.svelte` infix
  is mandatory. A file literally named `svelte.ts` is not a component and
  never triggers anything.
- `Foo.svelte.ts` + `Foo.svelte.js` **without** a bare `Foo.svelte` does
  not trigger: one side of the collision must be the component itself.
- Identically-named files in *different* directories
  (`invalid/cjs/Foo.svelte` vs `invalid/mjs/Foo.svelte`) are unrelated —
  same-directory only.

**Case sensitivity nuance (upstream behavior we should keep):** the base
name comparison is effectively case-insensitive *on case-insensitive
filesystems* (macOS/Windows), because the OS itself cannot distinguish
`foo.svelte.ts` from `Foo.svelte.ts` — the collision is real there.
Upstream therefore reports differently-cased collisions too, and re-reads
the directory (`realBasename`) only to get the display casing right in the
message. On Linux the same pair would not collide. Our Rust port should
mirror this: use the filesystem's own answer (`Path::exists` already
behaves per-FS), and use a `read_dir` fallback only for message casing.

### Detection strategy — per-file stat now, `notify`-driven re-checks later

This rule is designed for **two trigger strategies** sharing one pure check
function, because `oxvelte` will eventually have two execution modes:

- **Trigger A — lint-time stat (implemented now).** Upstream's approach,
  already the Rust-idiomatic shape for the one-shot CLI: the linter visits
  every file being linted. When linting `Foo.svelte`, the rule performs at
  most **8 `Path::exists` checks** (`Foo.svelte.{ts,tsx,js,jsx,mts,cts,mjs,
  cjs}`); when linting a module `Foo.svelte.ts`, one strip + one `exists`
  check on `Foo.svelte`. No directory enumeration, no globbing, no regex
  over directory listings — the only `read_dir` is the
  case-insensitive-casing fallback for the message text.
- **Trigger B — filesystem-event re-check (later, with the long-running
  mode).** A future VS Code extension (a separate project) will run
  `oxvelte` as a long-lived sub-process that reports diagnostics on the
  fly. In that mode, filename changes must re-trigger this rule nearly
  instantly no matter where the change came from (editor, terminal, Finder,
  any app). That is the job of the `notify` crate (v8.2.1, cross-platform
  native FS events): watch the directories derived from the `files` key in
  `oxvelte.config.json`, and on any create/rename/remove/move event
  affecting a `*.svelte*` name, re-run the rule's check for the affected
  directory and push updated diagnostics. No polling loops — the OS
  notifies us.

To make both triggers trivial, the rule's core is a **single pure function**
(e.g. `fn sibling_conflict(path: &Path) -> Option<Conflict>`, where
`Conflict` carries which side we're on plus the real-basename display
name). Trigger A calls it from `Rule::run` during a normal lint. Trigger B
(the future watch mode) calls it from the notify event handler for the
affected directory. The rule contains no watching code itself.

**Where the watcher lives (execution-flow research, 28OCT2026):** the
watching worker must NOT live inside this or any rule — rules are
parser-triggered per file and a watcher per rule wouldn't scale. The
current one-shot pipeline is:

1. `src/main.rs` clap subcommands: `Lint`, `Parse`, `Check`, `Rules`,
   `Migrate` (`main.rs:15-61`).
2. `cmd_lint` (`main.rs:171`): load config → build `Linter` →
   `collect_lint_files(paths)` (walks dirs, filters by extension,
   dedupes, `main.rs:856`) → rayon `par_iter` per file
   (`main.rs:230`): read source → `parser::parse_for_lint` (`.svelte`) or
   the script path → `lint.lint_with_project_config_and_path(...)`
   → collect diagnostics → report → exit.

So the watcher wraps this pipeline from **above**: a new library module
**`src/watch.rs`** owns the `notify` worker — one general worker, not
rule-specific. `src/main.rs` only wires a future service-mode CLI argument
(e.g. an `oxvelte watch`/`serve` subcommand next to `Lint`) to it. The
worker's event loop maps each notify event to a **re-lint unit**:

- A file event (create/write/rename/remove/move) for path `P`: re-lint
  `P` through the same per-file pipeline (`parser::parse_for_lint` →
  rules) — this triggers this rule along with all parser-driven rules, so
  the filename rule needs no special watch plumbing.
- **Sibling re-check (this rule's one watch-specific need):** a rename,
  move, or delete of `Foo.svelte.ts` must also clear/update the
  diagnostic previously reported on its collision partner `Foo.svelte`
  (and vice versa). On any event touching a `*.svelte*` name, the worker
  evaluates the `sibling_conflict` pure function for the event path AND
  re-lints whichever side of the pair exists on disk.
- Debounce/batching of bursty events (editors save in bursts) belongs to
  the worker, not the rule.

**Watch roots:** note that lint paths today come from CLI arguments, NOT
from a `files` key in `oxvelte.config.json` (the only `files` keys in the
config are `settings.svelte.kit.files.routes` and ESLint-legacy
`overrides[].files` globs — neither lists lint inputs). So the service
mode takes the same `paths` argument as `lint` and watches those roots
(`collect_lint_files`' directory set); a config-declared lint-paths key
would be a separate future feature.

**No async runtime needed:** `notify`'s recommended watcher delivers
events over a channel; a dedicated std thread with a `std::sync::mpsc`
loop suffices — no tokio dependency. (rayon is already used for parallel
lint; the watch worker is single-threaded per event batch, reusing the
same per-file lint call.)

**What lands in this workstream vs later:** the `notify` dependency (v8.2.1)
and the `src/watch.rs` worker — including its event→re-lint mapping and
sibling re-check, with unit tests — land NOW, so the engineering seam is
real and tested. The service-mode CLI argument in `main.rs` and the JSON
stdio protocol for the VS Code extension are explicitly **later** (the
extension is a separate project); until then the worker is exercised by
its tests, not dead production code.

### Exact message texts (parity requires verbatim)

- Component side: ``The module `Foo.svelte.ts` has the same name as this
  component. TypeScript resolves the import `./Foo.svelte` to that module,
  not to this component. Rename `Foo.svelte.ts`.``
- Module side: ``This module has the same name as the component
  `Foo.svelte`. TypeScript resolves the import `./Foo.svelte` to this
  module, not to the component. Rename this file.``

Report location: `Program`, line 1 column 0 (rendered as line 1, column 1).

## Fixture layout (why it's special)

`fixtures/linter/upstream/no-conflicting-module-names/{valid,invalid}/<case>/`
holds **real sibling files** (`Foo.svelte` + `Foo.svelte.ts`, etc.): 4
valid directories (+ 2 virtual-path cases in upstream's test source) and 16
invalid cases (8 extensions × component side + module side). Upstream's own
test suite bypasses its fixture loader for this rule and points the
RuleTester at the real on-disk files, because validity depends on
sibling-file existence.

**Manifest status (verified 28OCT2026):** the importer already inventories
all 23 files in `manifest.json`'s `files` list, but synthesizes **zero
executable `cases`** for the rule (the loader-style importer has no
expectation files to read). So no importer rework is needed for tracking —
but case entries with expectations do have to be added (step 4 below).

## What we need to build

1. **`src/linter/rules/no_conflicting_module_names.rs`** — new rule:
   - `name()` → `"svelte/no-conflicting-module-names"`;
     `is_recommended()` false (upstream default-off);
     `applies_to_svelte_scripts()` **true** so it also runs on
     `.svelte.js` / `.svelte.ts` modules (the Rule trait already supports
     this, `src/linter/mod.rs:265`; the linter already routes module files,
     `src/main.rs:247`).
   - In `run()`: return early when `ctx.file_path` is `None` or the file
     doesn't exist on disk (virtual/untitled inputs). Branch on
     `ctx.is_svelte_module` (`src/linter/mod.rs:82`):
     - **Component side** (`.svelte` file): for each of the 8 extensions,
       `Path::exists` on `<path><ext>`; on first hit report
       `conflictOnComponent` (with the real-basename casing lookup) and
       stop.
     - **Module side**: find which of the 8 extensions the filename ends
       with, strip it, and if the remainder ends with `.svelte` and exists
       on disk, report `conflictOnModule`.
   - Structure the logic as a pure, side-effect-light
     `sibling_conflict(path: &Path) -> Option<Conflict>` helper that
     `Rule::run` calls (Trigger A). The future watch mode (Trigger B) will
     call the same helper from its `notify` event handler — see Detection
     strategy. The rule itself contains no watching code.
   - Diagnostic at a zero-width span at offset 0 (verify it renders as
     line 1, column 1 through our span→loc conversion; adjust if our
     conventions require `0..1`).
   - Messages verbatim as above.
2. **Registration**: `mod` + `Box::new(...)` entry in
   `src/linter/rules/mod.rs` (following existing ordering).
3. **Unit tests in the rule file**: materialize temp directories
   (component + sibling module for each of the 8 extensions; `.d.ts`
   sibling; differently-named sibling; module-only pairs with no component;
   missing sibling; virtual/no-path lint) and lint with `file_path` set.
   Precedent for path-dependent rules:
   `no_export_load_in_svelte_module_in_kit_pages` uses `ctx.file_path` for
   Kit route detection.
4. **Fixture/parity wiring — verification plus case synthesis**:
   - Verify the manifest `files` inventory still matches after each
     `cargo get-eslint-fixtures` run (expected: already correct, additive).
   - Add the missing `cases` entries: in `scripts/get-fixtures/import.mjs`,
     synthesize cases for this rule from the directory layout — each
     `valid/<case>/<file>` a valid case; each `invalid/<ext>/<file>` an
     invalid case with exactly one expected error built from the upstream
     templates above (component side for `Foo.svelte`, module side for
     `Foo.svelte.<ext>`), line 1 / column 1. Keep it keyed off the rule
     directory name so re-imports stay idempotent.
   - Verify the lib test harness lints the `.svelte.ts` module-side files
     through the svelte-script path (`lint_fixture_file` in `src/lib.rs`
     must route non-`.svelte` extensions correctly — check before
     assuming).
   - The harness already passes the real on-disk relative path as
     `file_path` (`src/lib.rs:258,283`), so sibling detection works
     in-place — no tempdir copying needed in the harness.
5. **Docs**: check whether a rules table/list must include the new rule
   (README, docs/) and update `docs/upstream-parity.md` (lines 31 and 102
   currently describe this rule as not executed).
6. **`src/watch.rs` + `notify` dependency (v8.2.1)** — the general watch
   worker per the Detection strategy section: watcher setup over a set of
   root paths, an mpsc event loop with debounce, the event→re-lint mapping,
   and the sibling re-check driven by the rule's pure `sibling_conflict`
   function. One worker for the whole pipeline (parser + all rules), NOT
   rule-specific. Reuses the same per-file lint call as `cmd_lint`
   (extract that per-file body into a shared helper in `main.rs`/lib so
   `cmd_lint` and the watch worker can't drift). Unit tests: event→path
   mapping, debounce batching, sibling re-check on rename/delete. The
   service-mode CLI subcommand and the JSON stdio protocol are out of
   scope for this workstream (later, with the extension project).

## Validation

- `cargo test --lib` — new unit tests pass, no regressions.
- The lib linter-fixture tests for the rule
  (`run_linter_valid/invalid("no-conflicting-module-names")`) pass once
  manifest cases exist: all 16 invalid cases produce exactly one error each
  (both sides of all 8 extensions), all valid cases produce none.
- `cargo test --test upstream_parity` — the rule's cases leave the
  not-executed category and pass.
- `cargo clippy --all-targets` and `cargo fmt --check` clean.
- Real-world smoke: lint a scratch dir containing `Foo.svelte` +
  `Foo.svelte.ts` with the release binary and confirm both files report.

## Decisions recorded

- **Dual trigger design, one pure check, one general worker.** The rule's
  logic lives in a pure `sibling_conflict(path)` function called by
  Trigger A (lint-time `Path::exists` checks) and by Trigger B
  (`notify`-driven re-checks). Trigger B lands in this workstream as a
  general `src/watch.rs` worker (dependency: `notify` v8.2.1) that wraps
  the whole lint pipeline from above — parser-driven rules included —
  not inside any rule. The service-mode CLI argument and JSON stdio
  protocol are deferred to the extension project; the worker ships tested
  but not yet CLI-wired.
- **Keep upstream's case-insensitive filesystem semantics** — the collision
  is real wherever the OS can't distinguish the names.

## Open questions

- Zero-width span at offset 0: confirm our loc rendering matches upstream's
  line 1 / column 1 in the JSON/stylish output.
- Whether `Linter::all()` + the CLI's default ruleset should include a
  default-off rule (mirror upstream: available but not enabled by
  default — check how other `is_recommended() == false` rules are
  treated).
