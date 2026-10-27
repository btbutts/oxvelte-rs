# Proposal: pinned rule fixtures — one home, one automated update pipeline

Status: proposal, not started. This document is meant to be readable on its own,
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
  The script `scripts/upstream-parity/import.mjs` reads upstream's fixtures at
  one pinned commit and converts all of their expectation files into a single
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
   vendored upstream directories that nothing reads.
   The only exception to this are hand-copied rules that no longer have
   an upstream candidate, particularly within `eslint-plugin-svelte` as just
   because the upstream deletes a rule, does not mean that we automatically
   do.

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
differs from the commit-based plan; only the URL and the pin variables change.
The pins in `.cargo/config.toml` are therefore:

```toml
[env]
OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG = "3.23.1"
OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"
```

Bumping a fixture source = editing one version number in one file.

## Implementation sequence (chronological)

The work lands in two stages. Stage 1 is deliberately small: it builds all of
the shared machinery (launcher, downloader, sync/status helpers) against the
simpler of the two sources — the Svelte parser fixtures — and proves it
end-to-end before the ESLint fixture migration touches anything.

### Stage 1 — parser fixtures (`cargo get-svelte-fixtures`)

1. **`.cargo/config.toml`** — add `OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"`
   (and `OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG = "3.23.1"` now, so all
   pins live together from the start), the optional `OXVELTE_JS_RUNTIME`
   override comment, and the `[alias]` entries for both commands.
2. **`Cargo.toml`** — add the `[[bin]]` target for `launcher` (path
   `scripts/launcher.rs`).
3. **Rename `scripts/upstream-parity/` → `scripts/get-fixtures/`** — the
   directory's job is fetching fixtures. Update references that move with it
   (`docs/upstream-parity.md`, CI, `dev-setup.sh`). Make its `package.json`
   scripts runtime-neutral (shebang + `"./import.mjs"` entries — see
   "Running it" below).
4. **Write `scripts/launcher.rs`** — thin launcher: parse `--get-svelte-fixtures`
   / `--get-eslint-fixtures`, forward the matching `*_TAG` pin from the
   environment, select the JS runtime (bun first, node fallback,
   `OXVELTE_JS_RUNTIME` override), install `node_modules` in the script
   directory if absent, and execute `import.mjs` with cwd =
   `scripts/get-fixtures/`. No import logic in the launcher.
5. **Write `scripts/get-fixtures/github.mjs`** — the shared download /
   extract / sync helpers described below. Written generically from the start
   so Stage 2 reuses all of it unchanged.
6. **Extend `import.mjs`** — argument parsing plus the svelte job: fetch the
   `svelte@5.57.3` tarball, extract the two parser sample trees, sync them
   into `fixtures/parser/{legacy,modern}/` with per-rule status output.
7. **Validate Stage 1** — run `cargo get-svelte-fixtures`, then
   `cargo test --lib`. The first run doubles as a diff audit: the existing
   hand-copied parser fixtures should report mostly `SKIP`/`UPDATE`; any
   `CREATE` is a sample upstream added since the hand-copy, and the
   `no upstream candidate` list flags anything we hold that upstream renamed
   or dropped. Review that output before moving on.

### Stage 2 — ESLint rule fixtures (`cargo get-eslint-fixtures`)

1. **Step zero: the one-time dedupe** (manual — see its section below).
   Required before this stage, not before Stage 1.
2. **Extend `import.mjs`** with the eslint job: extract rule cases into
   `fixtures/linter/` (per-rule sync + status output), extract `src/rules/`
   to `tmp/` for metadata only, place `LICENSE` and the synthetic
   `package.json` at the fixture-home root, and update `manifest.json` with
   merge semantics (never regenerate wholesale).
3. **Repoint the test harnesses** — `tests/upstream_parity.rs` (`CORPUS` →
   `fixtures/linter`, `REVISION` reads the tag env var), `src/lib.rs` fixture
   tests read the manifest, `scripts/version-matrix/run.mjs` path repoint.
4. **Regenerate `tests/upstream-parity-baseline.json`** once
   (`--update-baseline`).
5. **Delete `fixtures/upstream/`** and run the full suite:
   `cargo test --lib`, `cargo test --test upstream_parity`, and the version
   matrix.

## Step zero: the one-time dedupe (manual, done once — Stage 2 prerequisite)

Before the eslint job runs, reconcile the two existing collections by hand:

1. For each rule folder in `fixtures/linter/`, check whether the same rule and
   cases exist in `fixtures/upstream/eslint-plugin-svelte/tests/fixtures/rules/`.
   Known state from earlier analysis: ~1,227 of the shared cases are
   content-identical (after CRLF→LF), upstream has ~69 cases we lack, and we
   hold **~55 local-only cases** that upstream has since deleted or never had.
2. Where a case exists in both, keep the **upstream copy** (our
   `-errors.yaml` files have already been corrected toward upstream, so the
   upstream expectations are the trusted ones).
3. Where a case exists only in `fixtures/linter/`, keep it in place — it stays
   as our copy of a rule upstream dropped (or a local regression case). These
   are precisely the cases goal #2 protects.
4. Where upstream has cases we lack, they arrive automatically on the first
   run of the new import pipeline — nothing to copy by hand.

After this step, `fixtures/linter/` is already the superset collection: every
upstream case we care about plus every local-only case we've retained,
including rules that may have been deleted/removed upstream in a later release.

## Target layout (after the pipeline lands)

```
fixtures/linter/                  # the ONE fixture home
  manifest.json                   # generated expectation index (updated, never regenerated wholesale)
  LICENSE                         # upstream eslint-plugin-svelte license
  package.json                    # synthetic: binds the Svelte version signal (replaces today's tests/package.json)
  <rule>/
    valid/   *.svelte (+ companions)
    invalid/ *.svelte, *-errors.yaml, *-config.json/_config.json, *-requirements.json, *-output.svelte
fixtures/parser/                  # parser AST samples from sveltejs/svelte (see below)
  legacy/<sample>/{input.svelte,output.json}
  modern/<sample>/{input.svelte,output.json}
```

Gone: `fixtures/upstream/eslint-plugin-svelte/` entirely — no `src/rules/`
TypeScript, no `tests/src/`, no `tests/utils/`, no second copy of the cases.

## The shared downloader: `scripts/get-fixtures/github.mjs`

Both fixture imports need the same capabilities, and both source repos ship the
same kind of tag tarball. All download/extract/sync logic lives in this one
module so `import.mjs` stays small and focused on processing (fixture handling
and manifest generation). Every function is generic — no repo names, no fixture
paths, no rule knowledge inside:

- **`fetch_github_release(owner, repo, tag)`** — downloads
  `https://github.com/<owner>/<repo>/archive/refs/tags/<repo>@<tag>.tar.gz`
  into a throwaway `tmp/` dir and returns the archive path. Same shape as the
  `fetch_github_repo` we already built in `build.rs`, just in JavaScript.
- **`extract_members(archivePath, selections)`** — extracts chosen members
  from the tarball to chosen destinations, where `selections` is a list of
  `{ members, dest }` pairs. Called as many times as needed, including
  multiple times against the same tarball when different file sets go to
  different destinations (Stage 1: parser-legacy samples →
  `fixtures/parser/legacy/`, parser-modern samples → `fixtures/parser/modern/`;
  Stage 2: rule cases → `fixtures/linter/`, `src/rules/` → `tmp/`, provenance
  files → the `fixtures/linter/` root).
- **`sync_rule_dirs(sourceDir, destDir, labelPrefix)`** — the per-rule copy
  and status engine (details in the next section). Works for both sources
  because both lay out rules the same way: one directory per rule, named
  after the rule, directly under the source root
  (`parser-legacy/samples/<name>/`, `tests/fixtures/rules/<name>/`).

`import.mjs` imports these; it owns no download/extract logic of its own.
Write the helpers with plain `for` loops over rule directories and file lists
(the same repetitive-structure style `import.mjs` already uses well), and keep
the whole module readable — the two jobs should share every code path except
their extraction selections, label prefixes, and (eslint only) the manifest
build.

> **Rename:** `scripts/upstream-parity/` becomes `scripts/get-fixtures/` when
> this lands — the directory's job is fetching fixtures, and it will hold
> both `import.mjs` and `github.mjs`. References elsewhere
> (`docs/upstream-parity.md`, CI, `dev-setup.sh`) move with it.

## Per-rule status reporting (both sources)

Every run prints one line per rule processed, then a closing block for local
rules the tarball didn't touch. Rule labels come from a small helper that maps
(source kind, rule directory name) to:

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
and incoming file contents). No hashing — the files are small (a parser rule
is just `input.svelte` + `output.json`), and we only need "different or not",
never what the difference is.

**Additive only, always:** a run can create or overwrite, never delete. After
the per-rule lines, print the local rules that had no counterpart in the
tarball:

```
Svelte Modern Parser - if-block: SKIP
Svelte Modern Parser - if-block-else: CREATE
Svelte Modern Parser - attachments: UPDATE
Svelte Legacy Parser - action: SKIP
Svelte Legacy Parser - animation: CREATE

rules with no upstream candidate:
  Svelte Legacy Parser - <name>
  ESLint Rule - <name>
  ...
```

The same output contract applies to the eslint job (`ESLint Rule - indent:
UPDATE`, etc.) — the only difference between the two jobs is the directory
depth between the source root and each rule folder, which the extraction
selections absorb.

## The updated `import.mjs` execution flow

`import.mjs` becomes argument-driven (invoked via the launcher, below) and
handles two jobs. Shared behavior: the pinned tag arrives as a forwarded
environment variable (from `.cargo/config.toml`) — the hardcoded
`export const revision = '...'` goes away, and the manifest's `revision`
field becomes the tag (e.g. `eslint-plugin-svelte@3.23.1`).

### Job 1: `--get-eslint-fixtures` (rule fixtures + manifest)

1. **Download.** `fetch_github_release("sveltejs", "eslint-plugin-svelte",
   <pinned tag>)` into `tmp/`.
2. **Extract rule cases and sync into the fixture home.** Extract
   `packages/eslint-plugin-svelte/tests/fixtures/rules/**` from the tarball,
   then `sync_rule_dirs(<extracted rules dir>, "fixtures/linter", "ESLint Rule")`
   applies per-rule semantics: a rule copied from upstream completely replaces
   the same-named local directory — same files, no extras, none missing — so
   upstream edits to a living rule arrive in kind on the next run. The
   tarball's `tests/fixtures/rules/` prefix maps onto the fixture-home root —
   the layouts are already identical, including `@typescript-eslint/`.
3. **Extract `src/rules/**` to `tmp/` only.** The TypeScript rule sources are
   needed *only* while building manifest entries (the `fixable` flag and
   Svelte-version gate per rule). They are read from `tmp/src/rules/` and
   discarded with `tmp/` — they never enter the repo. (Today's
   `const sourceFile = \`src/rules/${rule}.ts\`` lookup just repoints at the
   `tmp/` copy; the filenames are unchanged.)
4. **Extract provenance files to the fixture-home root.** Upstream's
   `LICENSE` lands at `fixtures/linter/LICENSE`. The synthetic
   `package.json` binding the pinned Svelte version (today's
   `tests/package.json`) is written to `fixtures/linter/package.json` — it is
   still needed: `svelte_version_info_for_file` (`src/linter/mod.rs`) walks
   up from a linted file to the nearest `package.json` declaring svelte, and
   the version-matrix runner rebuilds per-profile environments from it.
5. **Build the manifest over ALL of `fixtures/linter/`.** `import.mjs` keeps
   its current discovery logic **unchanged** — walk every rule directory,
   find the `valid`/`invalid` segments, discover inputs and companions
   (`_config.json` et al.) exactly as `buildManifest()` does today. The only
   change is the root it points at: `fixtures/linter/` instead of
   `tests/fixtures/rules/`. Because the root is the merged fixture home, the
   manifest covers upstream-refreshed rules, local-only rules, and the
   hand-built pseudo-rule folders (`valid-compile`, `valid-each-key`,
   `valid-prop-names-in-kit-pages`, `valid-style-parse`) — everything gets
   manifest entries, one schema for all.
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
7. **Print the per-rule status report** (format above) and **discard `tmp/`.**
   Nothing temporary survives the run.

After this job, `fixtures/upstream/eslint-plugin-svelte/` no longer exists and
`fixtures/linter/` is the single home both test suites read.

### Job 2: `--get-svelte-fixtures` (parser fixtures)

The parser samples in `fixtures/parser/` (currently a March 2026 hand-copy
from a deleted `prepare.sh`) become programmatic too:

1. `fetch_github_release("sveltejs", "svelte", <pinned tag>)` into `tmp/`.
2. Two `extract_members` selections from the same tarball:
   `packages/svelte/tests/parser-legacy/samples/**` and
   `packages/svelte/tests/parser-modern/samples/**`.
3. Two `sync_rule_dirs` calls: parser-legacy samples → `fixtures/parser/legacy/`
   with label prefix `Svelte Legacy Parser`, parser-modern samples →
   `fixtures/parser/modern/` with label prefix `Svelte Modern Parser`.
4. Print the per-rule status report (format above) and discard `tmp/`. No
   manifest — these fixtures are self-describing (`input.svelte` +
   `output.json` per sample) and are consumed by the `cargo test --lib`
   harnesses in `src/lib.rs` unchanged.

## Test-harness changes required

- `tests/upstream_parity.rs` — point `CORPUS` at `fixtures/linter` and read
  `fixtures/linter/manifest.json`. The hash-inventory check (`verify_files`)
  now covers the merged tree; the pinned-revision check and the baseline's
  `manifestHash`/`revision` coupling stay as they are, with `REVISION` read
  from `OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG` so the importer and the
  test suite can never disagree about which release is pinned.
- `src/lib.rs` fixture tests (`cargo test --lib`) — switch from reading each
  rule's loose `-errors.yaml` files to reading expectations from the same
  `manifest.json`, so both suites share one source of truth and one basic
  process: read the `.svelte` inputs from `fixtures/linter/`, validate
  against `fixtures/linter/manifest.json`. No separate flow for hand-built
  vs. imported rules.
- `scripts/version-matrix/run.mjs` — just a repoint: read the base manifest
  and fixture files from `fixtures/linter/` instead of
  `fixtures/upstream/eslint-plugin-svelte/`. Its per-profile behavior is
  unchanged: it still stamps a fresh synthetic `package.json` (signaling the
  profile's Svelte major to the linter under test) into each copied corpus —
  the base `fixtures/linter/package.json` is only the modern/Svelte-5 default
  that ships with the corpus.
- `tests/upstream-parity-baseline.json` — regenerate once after the migration
  (`--update-baseline`), since manifest hash and case identifiers move.

## Running it: cargo aliases, one place for pins

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
   - Otherwise probe PATH: **`bun` first, then `node`**. Both are
     Homebrew-installable; bun's runtime is faster, and users with only
     node/npm are never forced into a second ecosystem. macOS, Linux, and
     Windows must all work.
   - Ensure dependencies before running: if `scripts/get-fixtures/node_modules/`
     is absent, install from that directory first — `bun install` on the bun
     path, `npm ci` on the node path (the lockfile is npm's; bun doesn't
     read `package-lock.json`, and the dependencies are exact pins, so drift
     is not a concern).
   - Execute from the script's own directory (cwd =
     `scripts/get-fixtures/`): on the bun path
     `bun run --bun import.mjs --<job> …`; on the node path
     `node import.mjs --<job> …`. The launcher invokes the runtime binary
     directly on the file — it already knows which runtime it selected, so
     package.json scripts are not involved.

   The `--bun` flag matters because of bun's shebang deferral: by default bun
   respects a `#!/usr/bin/env node` shebang and defers to node; `--bun`
   forces Bun's own runtime. Same rule for package CLIs (`bunx --bun <pkg>`).

3. **`scripts/get-fixtures/package.json`** — the `import`/`test` scripts stay
   for CI and manual use, but become runtime-neutral: `import.mjs` gains a
   `#!/usr/bin/env node` shebang and the scripts call it directly
   (`"import": "./import.mjs"`, `"test": "…"` likewise). Then `npm run
   import` executes it with node (via the shebang) and `bun run --bun import`
   forces bun's runtime — the exact mechanism `--bun` exists for. Note:
   `$npm_execpath` is **not** usable as a runtime selector here — it points
   at the package-manager CLI (the bun binary under bun, but `npm-cli.js`
   under npm), so it works for `$npm_execpath install`-style commands but
   cannot run a JS file under both managers.

4. **Aliases** in `.cargo/config.toml`:

   ```toml
   [alias]
   get-eslint-fixtures = "run --bin launcher -- --get-eslint-fixtures"
   get-svelte-fixtures = "run --bin launcher -- --get-svelte-fixtures"
   ```

## What this gives us

- One fixture home (`fixtures/linter/`), one expectation index
  (`manifest.json`), two cargo commands to refresh either fixture source.
- Nothing upstream deletes can silently disappear from our support — the
  manifest merge preserves retired rules' metadata forever.
- No vendored dead weight: upstream's TypeScript rule sources and test suites
  never enter the repo; they are read from a throwaway `tmp/` dir or not
  downloaded at all.
- Parser fixtures stop being a fossilized hand-copy and become a pinned,
  refreshable import like everything else.
- After a pin move, a failing test always means "a rule's behavior changed
  upstream and our Rust code needs updating" — never "the fixture bookkeeping
  is stale."

## Runtime selection for the embedded bridge (planned)

The shipped `oxvelte` binary launches the four embedded `.cjs` scripts (see
`src/compiler.rs`) with a JavaScript runtime found on the user's machine. The
planned behavior: **prefer `bun` when it is on PATH, fall back to `node`** —
and `OXVELTE_NODE`-style explicit overrides keep working for users who want to
pin one. Both runtimes execute the bridge identically; bun (written in Rust
since 1.4, powered by JavaScriptCore) is the faster engine, and users who only
have node/npm are never forced into a second ecosystem. This must work on
macOS, Linux, and Windows. Today the lookup is `OXVELTE_NODE` or `node` on
PATH only, so the change is: probe for `bun` first, then `node`.

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

## Open questions — settled 25OCT2026

- **Rule deleted upstream → keep, automatically.** No prompt, no decision
  file, no comparison logic in `import.mjs`. The sync is additive-only:
  anything upstream removed simply isn't overwritten — it stays, and appears
  in the `no upstream candidate` list. Retiring one of our rules is always a
  manual act on our side.
- **Pseudo-rule folders (`valid-compile`, `valid-each-key`,
  `valid-prop-names-in-kit-pages`, `valid-style-parse`) → manifest entries
  like everything else.** The manifest covers ALL of `fixtures/linter/`, so
  the Rust tests have exactly one process: read inputs from
  `fixtures/linter/`, validate against `fixtures/linter/manifest.json`.
- **Layout mapping → identity.** Upstream's `tests/fixtures/rules/<rule>/
  {valid,invalid}/...` and our `fixtures/linter/<rule>/{valid,invalid}/...`
  follow the same schema (including `@typescript-eslint/`), so the sync is a
  straight per-rule-directory overwrite with no remapping.
- **`src/rules/*.ts` lookup → repoint to `tmp/`.** The metadata reader keeps
  using the rule name (`src/rules/${rule}.ts`) against the `tmp/` extraction;
  filenames are unchanged, only the root differs.
- **`scripts/version-matrix/run.mjs` → path repoint only.** It reads the
  manifest and synthetic `package.json` from `fixtures/linter/`; nothing
  about its flow changes.
- **Pins → release tags (settled 26OCT2026).** Both sources are pinned by
  release tag (`*_TAG` env vars), downloaded as GitHub tag source archives.
  Same tarball mechanics as a commit pin, friendlier to read and bump.
