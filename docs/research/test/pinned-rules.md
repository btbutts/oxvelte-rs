# Proposal: pinned rule fixtures — one home, one automated update pipeline

Status: landed. Stage 1 (parser-fixture pipeline) and Stage 2 (eslint rule
fixtures) both landed 27OCT2026; follow-up corrections in
`docs/research/test/oxc-upgrade-and-fixtures-corrections_27OCT2026.md` landed
the same day. This document is meant to be readable on its own,
without knowing the codebase first.

## Background: why we have two fixture collections today

Our linter is a drop-in replacement for `eslint-plugin-svelte`, so every rule we
ship must produce the same messages at the same positions as that upstream
project. To prove that, we keep upstream's example files (called fixtures) and
their expected results, and our test suite compares our output against them.

Today those fixtures live in two places:

- `fixtures/linter/` — an old, hand-copied snapshot of upstream's fixtures
  (March 2026, from the original repo era). Each rule has its own folder with
  `valid/` examples (the rule must stay silent) and `invalid/` examples (the
  rule must fire), plus `-errors.yaml` files that record the expected message,
  line, and column for each finding. These are checked by `cargo test --lib`.
  Eleven-plus of its `-errors.yaml` files have already been hand-corrected to
  match upstream's current expectations.
- `fixtures/upstream/eslint-plugin-svelte/` — a newer, machine-generated copy.
  The script `scripts/get-fixtures/import.mjs` (renamed from
  `scripts/upstream-parity/` in Stage 1) reads upstream's fixtures at one
  pinned commit and converts all of their expectation files into a single
  `manifest.json`. These are checked by `cargo test --test upstream_parity`.

The second collection was built next to the first instead of replacing it, so
the two mostly duplicate each other. Nothing user-facing depends on either
copy — these files are only test data.

One more fact that matters for everything below: **a fixture is only a test.**
A rule works because a Rust file implements it. Fixtures prove the rule works;
they do not provide the rule.

### What the machine-generated half actually contains (verified 24OCT2026)

`fixtures/upstream/eslint-plugin-svelte/` is a partial mirror of the upstream
repo. Only some of it is load-bearing:

- `tests/fixtures/rules/**` — the real test corpus: the `.svelte` example
  inputs plus their `-errors.yaml`, `-config.json`, `-requirements.json`, and
  `-output.svelte` companions. **Required.**
- `src/rules/*.ts` — upstream's TypeScript rule implementations. `import.mjs`
  reads them at import time for exactly two facts per rule (whether it is
  `fixable`, and whether it carries a Svelte-version gate) and writes those
  facts into `manifest.json`. **Nothing else ever reads them** — the Rust
  tests never touch them. They do not need to live in our tree.
- `tests/src/**`, `tests/utils/**` (including `index.spec.ts`, `meta.ts`) —
  upstream's own TypeScript test suites. **Completely unused by us**; imported
  only because the importer mirrors the whole `tests/` directory.
- `manifest.json` — the generated index: pinned revision, environment
  versions, every case's expectations (message/line/column, config,
  requirements, expected fix output), and a sha256 of every corpus file.
  **Required** — this is what `tests/upstream_parity.rs` validates against.
  Note: the manifest stores *expectations*, not inputs — the `.svelte` files
  themselves are still read from disk by the test (`upstream_parity.rs` reads
  each case's `filename` from the corpus tree). So both the fixture files and
  the manifest are needed at test time.

## Goals

1. Keep tracking upstream `eslint-plugin-svelte` as it evolves, by moving the
   pinned release tag forward when we choose to. We do the same for the
   `sveltejs/svelte` parser test files.
2. Never silently lose rules or test cases that upstream removes but we want
   to keep. Deleting one of our rules when upstream deletes theirs is our
   decision, never a byproduct of theirs.
3. Store each test case in exactly one place.
4. Make the whole update a single command, after one manual cleanup.
5. Source everything programmatically — no hand-copied fixture trees, no
   vendored upstream directories that nothing reads. The only exception is
   `fixtures/linter/local/`: our own cases that upstream never had (or dropped)
   are kept by hand, deliberately, and are never written by the importer.
6. Never hand-edit an upstream fixture to make our output match. If our linter
   disagrees with an upstream expectation, the test goes red and the fix lands
   in our Rust code — editing the expectation would defeat the point of being
   a drop-in replacement.

## Pins: release tags, not commit hashes

Both upstream repos publish proper release tags, and GitHub serves a source
archive for any tag at a predictable URL:

```
https://github.com/<owner>/<repo>/archive/refs/tags/<repo>@<tag>.tar.gz
```

Concretely (both verified to resolve, 26OCT2026):

```
https://github.com/sveltejs/eslint-plugin-svelte/archive/refs/tags/eslint-plugin-svelte@3.23.1.tar.gz
https://github.com/sveltejs/svelte/archive/refs/tags/svelte@5.57.3.tar.gz
```

These are the same on-the-fly source archives GitHub generates for a commit
hash — just addressed by tag. Nothing about downloading or extracting them
differs from a commit pin; only the URL and the pin variables change.
The pins in `.cargo/config.toml` (landed in Stage 1):

```toml
[env]
OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG = "3.23.1"
OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"
```

Bumping a fixture source = editing one version number in one file.

## Implementation sequence (chronological)

The work lands in two stages. Stage 1 built all of the shared machinery
(launcher, downloader, sync/status helpers) against the simpler of the two
sources — the Svelte parser fixtures — and proved it end-to-end before the
ESLint fixture migration touches anything.

### Stage 1 — parser fixtures (`cargo get-svelte-fixtures`) — LANDED 27OCT2026

1. **`.cargo/config.toml`** — `OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"`,
   `OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG = "3.23.1"`, the optional
   `OXVELTE_JS_RUNTIME` override comment, and the `[alias]` entries.
2. **`Cargo.toml`** — `[[bin]] launcher` (path `scripts/launcher.rs`).
3. **Renamed `scripts/upstream-parity/` → `scripts/get-fixtures/`** —
   references moved with it (`.gitignore`, `ci.yml`, `dev-setup.sh`,
   `docs/upstream-parity.md`, the version-matrix doc); `package.json` renamed
   to `oxvelte-get-fixtures` and its `import` script made runtime-neutral
   (`./import.mjs` + shebang).
4. **`scripts/launcher.rs`** — thin launcher: parses the job flag, forwards
   the matching `*_TAG` pin, selects the JS runtime (bun first, node fallback,
   `OXVELTE_JS_RUNTIME` override), installs `node_modules` in the script
   directory if absent, executes `import.mjs` with cwd =
   `scripts/get-fixtures/`.
5. **`scripts/get-fixtures/github.mjs`** — `fetchGithubRelease`,
   `extractMembers`, `syncRuleDirs` (see its section below).
6. **`import.mjs`** — argument-driven; the svelte job works;
   `--get-eslint-fixtures` errors until Stage 2. The legacy
   git-checkout import path was removed; `import.test.mjs` still passes (6/6).
7. **Validation result** — the pipeline runs correctly, but syncing at the
   5.57.3 pin turns 26 parser fixture tests red: the old hand-copy matches
   *our serializer*, not any released svelte version (verified by bisecting
   5.40.0 → 5.57.3). Fixtures were restored to the old copy (suite green,
   624 passing); the parser-parity workstream is tracked in
   `docs/research/parser/parser-parity.md` and must land before
   `cargo get-svelte-fixtures` output is ever committed.

### Stage 2 — ESLint rule fixtures (`cargo get-eslint-fixtures`)

1. **Step zero: the one-time dedupe** (manual — see its section below).
2. **Extend `import.mjs`** with the eslint job: extract rule cases into
   `fixtures/linter/upstream/` (per-rule sync + status output), extract
   `src/rules/` to `tmp/` for metadata only, place `LICENSE` and the synthetic
   `package.json` at the fixture-home root, and update `manifest.json` over
   BOTH roots (`upstream/` + `local/`) with merge semantics (never regenerate
   wholesale).
3. **Repoint the test harnesses** — `tests/upstream_parity.rs` (`CORPUS` →
   `fixtures/linter`, `REVISION` reads the tag env var), `src/lib.rs` fixture
   tests read the manifest, `scripts/version-matrix/run.mjs` path repoint.
4. **Regenerate `tests/upstream-parity-baseline.json`** once
   (`--update-baseline`).
5. **Delete `fixtures/upstream/`** and run the full suite:
   `cargo test --lib`, `cargo test --test upstream_parity`, and the version
   matrix.

## Step zero: the one-time dedupe (manual, done once — Stage 2 prerequisite)

Dedupe analysis (run 27OCT2026, CRLF-normalized): 78 local rule dirs, 84
upstream. **No local-only rules** — every local rule exists upstream; 6
upstream-only rules we gain automatically. 42 shared rules are byte-identical.
36 shared rules differ, of which 33 differ **only by extra local-only case
files** (~100 files of our own regression cases) and 3 have changed shared
files (`button-has-type`, `valid-style-parse`,
`valid-prop-names-in-kit-pages`).

The manual reconciliation:

1. **Relocate the local-only case files.** For each of the ~33 rules with
   extras, move the local-only files from `fixtures/linter/<rule>/...` into
   the mirrored path under `fixtures/linter/local/<rule>/...` (e.g.
   `fixtures/linter/no-dupe-on-directives/invalid/token-equivalence-*` →
   `fixtures/linter/local/no-dupe-on-directives/invalid/token-equivalence-*`).
   Mechanical, script-assisted, reviewed via git diff. Once done,
   `import.mjs` never writes to `local/` again.
2. **Changed shared files: upstream wins.** Our variants of files that exist
   at the same path in both trees are discarded (goal #6 — we do not fork
   upstream expectations; a red test means our Rust code needs work). No
   copying needed: the first sync brings upstream's versions.
3. **Identical rules need nothing** — the first sync recreates them under
   `fixtures/linter/upstream/`.
4. **Delete the old tree.** Once `fixtures/linter/upstream/` is populated and
   the suite passes, the old `fixtures/linter/<rule>/` directories and
   `fixtures/upstream/` are deleted.

After this step, every case lives in exactly one of the two roots.

## Target layout (after the pipeline lands)

```
fixtures/linter/                  # the ONE fixture home
  manifest.json                   # generated expectation index (updated, never regenerated wholesale)
  LICENSE                         # upstream eslint-plugin-svelte license
  package.json                    # synthetic: binds the Svelte version signal (replaces today's tests/package.json)
  upstream/                       # byte-identical mirror of upstream's tests/fixtures/rules/ — sync target
    <rule>/
      valid/   *.svelte (+ companions)
      invalid/ *.svelte, *-errors.yaml, *-config.json/_config.json, *-requirements.json, *-output.svelte
  local/                          # our own cases, same layout, hand-maintained — import.mjs never writes here
    <rule>/
      valid/   *.svelte (+ companions)
      invalid/ *.svelte + companions
fixtures/parser/                  # parser AST samples from sveltejs/svelte (see below)
  legacy/<sample>/{input.svelte,output.json}
  modern/<sample>/{input.svelte,output.json}
```

Gone: `fixtures/upstream/eslint-plugin-svelte/` entirely — no `src/rules/`
TypeScript, no `tests/src/`, no `tests/utils/`, no second copy of the cases.

Because `local/` mirrors the upstream layout exactly, the rule name of every
case is its path relative to the root (`local/block-lang/invalid/x` → rule
`block-lang` → `svelte/block-lang` in the runner) — no new rule names, no
mapping table, and the manifest can treat the two roots as one additive
corpus.

## The shared downloader: `scripts/get-fixtures/github.mjs` (landed Stage 1)

Both fixture imports need the same capabilities, and both source repos ship the
same kind of tag tarball. All download/extract/sync logic lives in this one
module so `import.mjs` stays small and focused on processing (fixture handling
and manifest generation). Every function is generic — no repo names, no fixture
paths, no rule knowledge inside:

- **`fetchGithubRelease(owner, repo, tag, destDir)`** — streams
  `https://github.com/<owner>/<repo>/archive/refs/tags/<repo>@<tag>.tar.gz`
  into a throwaway temp dir and returns the archive path.
- **`extractMembers(archivePath, destDir, prefixes)`** — extracts regular
  files whose root-stripped path starts with one of `prefixes` into `destDir`,
  preserving structure. Called as many times as needed, including multiple
  times against the same tarball when different file sets go to different
  destinations. (Dependency-free tar.gz reader: ustar + GNU longname + pax.)
- **`syncRuleDirs(sourceDir, destDir, labelPrefix)`** — the per-rule copy
  and status engine (details in the next section). Works for both sources
  because both lay out rules the same way: one directory per rule, named
  after the rule, directly under the source root
  (`parser-legacy/samples/<name>/`, `tests/fixtures/rules/<name>/`).

`import.mjs` imports these; it owns no download/extract logic of its own.

## Per-rule status reporting (both sources)

Every run prints one line per rule processed, then a closing block for local
rules the tarball didn't touch. Rule labels:

- `Svelte Legacy Parser - <name>` (parser-legacy samples)
- `Svelte Modern Parser - <name>` (parser-modern samples)
- `ESLint Rule - <name>` (eslint-plugin-svelte rules)

Statuses:

- **`CREATE`** — the rule directory doesn't exist locally yet: create it and
  copy the rule's files in. No comparison needed.
- **`UPDATE`** — the rule exists locally and at least one file differs:
  overwrite so the local rule directory ends up byte-identical to the source.
  For eslint rules this is whole-rule granularity — one changed file anywhere
  under `<rule>/` refreshes the entire rule directory and prints a single
  line; we never report individual files.
- **`SKIP`** — every file in the rule is identical; nothing written.

Change detection is a direct byte comparison (`Buffer.equals` on the existing
and incoming file contents). No hashing — the files are small and we only need
"different or not", never what the difference is.

**Additive only, always:** a run can create or overwrite, never delete. The
closing "rules with no upstream candidate" block lists directories in the
**sync target** with no counterpart in the tarball — for the eslint job that
means `fixtures/linter/upstream/` only (a rule upstream later deletes stays
and is listed); `fixtures/linter/local/` is never scanned, since it is ours
by definition.

```
Svelte Modern Parser - if-block: SKIP
Svelte Modern Parser - if-block-else: CREATE
Svelte Modern Parser - attachments: UPDATE
Svelte Legacy Parser - action: SKIP
Svelte Legacy Parser - animation: CREATE

rules with no upstream candidate:
  Svelte Legacy Parser - <name>
  ...
```

## The updated `import.mjs` execution flow

`import.mjs` is argument-driven (invoked via the launcher, below) and handles
two jobs. Shared behavior: the pinned tag arrives as a forwarded environment
variable (from `.cargo/config.toml`); the manifest's `revision` field becomes
the tag (e.g. `eslint-plugin-svelte@3.23.1`).

### Job 1: `--get-eslint-fixtures` (rule fixtures + manifest) — Stage 2

1. **Download.** `fetchGithubRelease("sveltejs", "eslint-plugin-svelte",
   <pinned tag>, tmp)` — into an OS temp dir.
2. **Extract rule cases and sync into the upstream root.** Extract
   `packages/eslint-plugin-svelte/tests/fixtures/rules/**` from the tarball,
   then `syncRuleDirs(<extracted rules dir>, "fixtures/linter/upstream",
   "ESLint Rule")` applies per-rule semantics: a rule copied from upstream
   completely replaces the same-named directory there — same files, no
   extras, none missing — so upstream edits to a living rule arrive in kind
   on the next run, and the directory stays byte-identical to upstream.
   `fixtures/linter/local/` is untouched.
3. **Extract `src/rules/**` to the temp dir only.** The TypeScript rule
   sources are needed *only* while building manifest entries (the `fixable`
   flag and Svelte-version gate per rule). They are read from
   `<tmp>/src/rules/` and discarded with the temp dir — they never enter the
   repo. (Today's `const sourceFile = \`src/rules/${rule}.ts\`` lookup just
   repoints at the temp copy; the filenames are unchanged.)
4. **Extract provenance files to the fixture-home root.** Upstream's
   `LICENSE` lands at `fixtures/linter/LICENSE`. The synthetic
   `package.json` binding the pinned Svelte version (today's
   `tests/package.json`) is written to `fixtures/linter/package.json` — it is
   still needed: `svelte_version_info_for_file` (`src/linter/mod.rs`) walks
   up from a linted file to the nearest `package.json` declaring svelte, and
   the version-matrix runner rebuilds per-profile environments from it.
5. **Build the manifest over BOTH roots.** `import.mjs` keeps its current
   discovery logic — walk rule directories, find the `valid`/`invalid`
   segments, discover inputs and companions (`_config.json` et al.) exactly
   as `buildManifest()` does today — but applied to `fixtures/linter/upstream/`
   and `fixtures/linter/local/` as one additive corpus:
   - a case's **rule** is its path relative to the root (the root segment
     `upstream/` or `local/` is stripped), so `local/block-lang/...` cases map
     to `svelte/block-lang` exactly like upstream's do;
   - a case's **id** stays root-stripped (`block-lang/invalid/foo-input.svelte`)
     for continuity with today's ids — collisions are impossible because a
     case lives in exactly one root;
   - **filename** (what the Rust test reads from disk) and the **files hash
     inventory** use the full path including the root segment.
   The manifest covers upstream-refreshed rules, every `local/` case, and the
   hand-built pseudo-rule folders (`valid-compile`, `valid-each-key`,
   `valid-prop-names-in-kit-pages`, `valid-style-parse`) — which live under
   `local/` after step zero — one schema for all.
6. **Update the manifest — never regenerate it wholesale, and never delete
   rules.** The merge rules are simple:
   - Cases present in the fresh upstream archive: entries are **updated** (or
     added, for new rules/cases).
   - Rules absent from the archive: **kept automatically, no prompt.** The
     importer is deliberately dumb about upstream deletions — it never
     compares the tree against the archive to prune anything. Entries for
     kept rules are **preserved as-is**, including the `fixable` flag and
     version gate that can no longer be recomputed without the TypeScript
     sources. Git history is the safety net: the manifest diff is reviewable
     and restorable. If we ever choose to retire a rule upstream dropped,
     that's a manual act on our side (delete the folder and the Rust rule) —
     never something `import.mjs` does for us.
7. **Print the per-rule status report** (format above) and **discard the temp
   dir.** Nothing temporary survives the run.

After this job, `fixtures/upstream/eslint-plugin-svelte/` no longer exists and
`fixtures/linter/` is the single home both test suites read.

### Job 2: `--get-svelte-fixtures` (parser fixtures) — landed Stage 1

The parser samples in `fixtures/parser/` (a March 2026 hand-copy from a
deleted `prepare.sh`) are now programmatic. **Blocked on parser parity:** the
hand-copy matches our serializer, not any upstream release, so syncing at the
5.57.3 pin turns the parser fixture tests red until our parser is brought in
line — see `docs/research/parser/parser-parity.md`. Do not re-run this job
and commit the result until that workstream lands.

Mechanics (working): `fetchGithubRelease("sveltejs", "svelte", <pinned tag>)`,
one `extractMembers` call covering
`packages/svelte/tests/parser-legacy/samples/` and
`packages/svelte/tests/parser-modern/samples/`, then one `syncRuleDirs` call
per suite into `fixtures/parser/{legacy,modern}/` with the matching label
prefix, status report, temp dir discarded. No manifest — these fixtures are
self-describing (`input.svelte` + `output.json` per sample) and are consumed
by the `cargo test --lib` harnesses in `src/lib.rs` unchanged.

## Test-harness changes required

- `tests/upstream_parity.rs` — point `CORPUS` at `fixtures/linter` and read
  `fixtures/linter/manifest.json`. The hash-inventory check (`verify_files`)
  now covers the merged tree (both roots); the pinned-revision check and the
  baseline's `manifestHash`/`revision` coupling stay as they are, with
  `REVISION` read from `OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG` so the
  importer and the test suite can never disagree about which release is
  pinned. The runner's rule mapping (`svelte/<rule>`, `@`-namespaces) is
  unchanged — manifest `rule` fields are root-stripped, so no mapping changes
  are needed for `local/` cases.
- `src/lib.rs` fixture tests (`cargo test --lib`) — switch from reading each
  rule's loose `-errors.yaml` files to reading expectations from the same
  `manifest.json`, so both suites share one source of truth and one basic
  process: read the `.svelte` inputs from `fixtures/linter/`, validate
  against `fixtures/linter/manifest.json`. No separate flow for hand-built
  vs. imported rules.
- `scripts/version-matrix/run.mjs` — just a repoint: read the base manifest
  and fixture files from `fixtures/linter/` instead of the old corpus. Its
  per-profile behavior is unchanged: it still stamps a fresh synthetic
  `package.json` (signaling the profile's Svelte major to the linter under
  test) into each copied corpus — the base `fixtures/linter/package.json` is
  only the modern/Svelte-5 default that ships with the corpus. (Landed in
  Stage 2: eligibility recomputation per profile uses the manifest's
  machine-readable `gates` field instead of upstream's TypeScript sources.)
  Open → decided 27OCT2026: the two cross-rule `local/` cases are excluded
  from the matrix via a named, commented skip list in `run.mjs` — see
  `oxc-upgrade-and-fixtures-corrections_27OCT2026.md`, Workstream B.
- `tests/upstream-parity-baseline.json` — regenerate once after the migration
  (`--update-baseline`), since manifest hash and case identifiers move.

## Running it: cargo aliases, one place for pins (landed Stage 1)

1. **Environment variables in `.cargo/config.toml`** (alongside the existing
   `OXVELTE_CSS_SRC_*` pins) — release-tag pins and the runtime override only;
   the script path is *not* a variable, since we never expect it to change:

   ```toml
   [env]
   OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG = "3.23.1"
   OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"
   # Optional runtime override: "bun" or "node". Unset = auto-detect (bun preferred).
   # OXVELTE_JS_RUNTIME = "node"
   ```

   Every external pin we track — CSS sources, rule fixtures, parser
   fixtures — lives in this one file.

2. **`scripts/launcher.rs`** — a thin `[[bin]]` that parses its argument,
   forwards the matching pin from the environment, and starts
   `scripts/get-fixtures/import.mjs` (a static path compiled into the
   launcher — no env var) with the right runtime. It contains no import
   logic. Runtime selection:

   - If `OXVELTE_JS_RUNTIME` is set, use exactly that.
   - Otherwise probe PATH: **`bun` first, then `node`**. macOS, Linux, and
     Windows must all work.
   - Ensure dependencies before running: if `scripts/get-fixtures/node_modules/`
     is absent, install from that directory first — `bun install` on the bun
     path, `npm ci` on the node path (the lockfile is npm's; bun doesn't
     read `package-lock.json`, and the dependencies are exact pins, so drift
     is not a concern).
   - Execute from the script's own directory (cwd =
     `scripts/get-fixtures/`): on the bun path
     `bun run --bun import.mjs --<job>`; on the node path
     `node import.mjs --<job>`.

   The `--bun` flag matters because of bun's shebang deferral: by default bun
   respects a `#!/usr/bin/env node` shebang and defers to node; `--bun`
   forces Bun's own runtime. Same rule for package CLIs (`bunx --bun <pkg>`).

3. **`scripts/get-fixtures/package.json`** — the `import`/`test` scripts stay
   for CI and manual use; `import` is runtime-neutral (`./import.mjs` with a
   `#!/usr/bin/env node` shebang). `$npm_execpath` is **not** usable as a
   runtime selector — it points at the package-manager CLI, not a JS runtime.

4. **Aliases** in `.cargo/config.toml`:

   ```toml
   [alias]
   get-eslint-fixtures = "run --bin launcher -- --get-eslint-fixtures"
   get-svelte-fixtures = "run --bin launcher -- --get-svelte-fixtures"
   ```

## What this gives us

- One fixture home (`fixtures/linter/`), one expectation index
  (`manifest.json`), two cargo commands to refresh either fixture source.
- A byte-identical mirror of upstream (`upstream/`) we never hand-edit, side
  by side with our own cases (`local/`) the importer never touches — both
  validated by the same manifest and the same test process.
- Nothing upstream deletes can silently disappear from our support — the
  manifest merge preserves retired rules' metadata forever.
- No vendored dead weight: upstream's TypeScript rule sources and test suites
  never enter the repo; they are read from a throwaway temp dir or not
  downloaded at all.
- After a pin move, a failing test always means "a rule's behavior changed
  upstream and our Rust code needs updating" — never "the fixture bookkeeping
  is stale."

## Runtime selection for the embedded bridge (landed 27OCT2026)

The shipped `oxvelte` binary launches the four embedded `.cjs` scripts (see
`src/compiler.rs`) with a JavaScript runtime found on the user's machine.
Resolution order in `resolve_js_runtime` (`src/compiler.rs`):

1. The `OXVELTE_NODE` environment override (name or absolute path) — the
   long-standing escape hatch, unchanged.
2. The project config's `settings["js-runtime"]`: a `"bun"`/`"node"` string
   forcing that runtime from PATH, or a `["bun"|"node", {"path": "..."}]`
   pair pinning an explicit binary (the name is required alongside the path
   so no fragile binary sniffing is involved).
3. PATH probing: **`bun` first, then `node`**. If neither is found, the
   error tells the user to install one or set an override.

Both runtimes execute the same bootstrap unchanged (verified:
`bun --input-type=commonjs -e` runs the eval bootstrap and CommonJS
`require` identically). bun (written in Rust since 1.4, powered by
JavaScriptCore) is the faster engine, and users who only have node/npm are
never forced into a second ecosystem. The full fixture suite, including the
compiler-backed and type-aware cases, passes under bun.

## Deferred ideas (recorded 24OCT2026, not part of this plan)

- Relocate `scripts/compiler-runtime/` and possibly `scripts/version-matrix/`
  out of `scripts/` — the compiler runtime is embedded into the shipped binary
  via `include_str!`, so it is a runtime component, not a development script.
  Candidate home: `resources/runtimes/compiler-runtime/`. (Note: the source
  release workflow hard-codes the four embedded file paths, and `include_str!`
  paths in `src/compiler.rs` would move in lockstep.)
- Runtime selection beyond the binary and the fixture tooling: routing the
  remaining *development* scripts (npm ci in dev-setup, node --test, the
  various .mjs tools) through `bun` instead of `node`. Deferred — the
  binary-side bun preference above and the launcher's bun-first execution of
  `import.mjs` are the planned parts; the rest of dev tooling stays on
  node/npm until we choose otherwise.
- Type-aware checking in Rust via `tsrs` ([oxc-project/tsrs](https://github.com/oxc-project/tsrs),
  the Rust port of the TypeScript 7 type checker): for projects on TypeScript
  6+, route type-aware rules (today `no-unnecessary-condition`, via
  `typescript-service.cjs` + `typescript-conditions.cjs`) to an embedded
  `tsrs` host instead of the Node sidecar. Rationale: TS6 is the bridge
  release — a project compiling cleanly on TS6 is TS7-safe, and the type
  system is continuous between 6 and 7 — so tsrs's TS7-conformant checker is
  valid for TS6+ projects. The sidecar stays for TS ≤ 5 projects (our matrix
  verifies typescript-service.cjs back to 5.2.2). **Design settled — see
  `docs/research/dependency/tsrs-type-aware-checking.md`.** Summary: optional
  Cargo features (`tsrs-inference` = support code, `tsrs-inference-build` =
  compile the pinned tsrs binary at build time); source/homebrew builds
  compile tsrs as a sidecar binary; npm/bun installs get it as a package
  dependency, never compiled; a `settings.tsrs` boolean in
  `oxvelte.config.json` toggles the tsrs path at runtime. Related but
  independent: our `oxc` pin (`=0.128.0`) is old; an upgrade (e.g. toward
  0.155.x) is its own workstream.

## Open questions — settled

- **Rule deleted upstream → keep, automatically (25OCT2026).** No prompt, no
  decision file, no comparison logic in `import.mjs`. The sync is
  additive-only: anything upstream removed simply isn't overwritten — it
  stays, and appears in the `no upstream candidate` list. Retiring one of our
  rules is always a manual act on our side.
- **Pseudo-rule folders (`valid-compile`, `valid-each-key`,
  `valid-prop-names-in-kit-pages`, `valid-style-parse`) → manifest entries
  like everything else (25OCT2026).** The manifest covers ALL of
  `fixtures/linter/` (both roots), so the Rust tests have exactly one
  process: read inputs from `fixtures/linter/`, validate against
  `fixtures/linter/manifest.json`.
- **Local-only cases inside shared upstream rules → split roots
  (27OCT2026).** The dedupe analysis found ~100 local-only case files inside
  33 rules that also exist upstream, which whole-rule byte-identical sync
  would have deleted. Resolution: two sibling roots —
  `fixtures/linter/upstream/` (byte-identical sync target, never hand-edited)
  and `fixtures/linter/local/` (our cases, same layout, importer never
  writes). Rule names derive from the root-relative path, so the runner needs
  no mapping changes.
- **Layout mapping → identity (25OCT2026).** Upstream's
  `tests/fixtures/rules/<rule>/{valid,invalid}/...` and our
  `fixtures/linter/{upstream,local}/<rule>/{valid,invalid}/...` follow the
  same schema (including `@typescript-eslint/`), so the sync is a straight
  per-rule-directory overwrite with no remapping.
- **`src/rules/*.ts` lookup → repoint to the temp dir (25OCT2026).** The
  metadata reader keeps using the rule name (`src/rules/${rule}.ts`) against
  the temporary extraction; filenames are unchanged, only the root differs.
- **`scripts/version-matrix/run.mjs` → path repoint only (25OCT2026).** It
  reads the manifest and synthetic `package.json` from `fixtures/linter/`;
  nothing about its flow changes.
- **Pins → release tags (26OCT2026).** Both sources are pinned by release tag
  (`*_TAG` env vars), downloaded as GitHub tag source archives. Same tarball
  mechanics as a commit pin, friendlier to read and bump.
