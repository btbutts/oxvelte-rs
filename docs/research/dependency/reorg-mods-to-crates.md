# Reorganizing `src/` modules into a workspace of internal crates — plan

Status: proposed 02NOV2026; **phases 1–6 complete 02NOV2026** —
phase 1: `[workspace]`/`[workspace.package]`/`[workspace.dependencies]` in
the root `Cargo.toml`, all root deps inheriting, `resolver = "2"`, version
literal moved to `[workspace.package]`, `release.sh` on the perl
`get_version()`/`set_version()` helpers with the post-write verification
gate. Phase 2: `crates/oxvelte_transport` created (`transport/` moved as
`lib.rs` + `message_pack.rs`, plus the new `span.rs` holding
`byte_utf16`/`utf16_byte` and their test, moved verbatim from
`compiler.rs`). Phase 3: `crates/oxvelte_parser` created (`parser/` +
`ast.rs` as `pub mod ast`; `scanner` widened to `#[doc(hidden)] pub` for
`compiler_ignore`; internal `crate::parser::` paths became `crate::`;
`rustc_hash`/`precomputed_hash` deps added — used via full paths, so the
`use`-grep had missed them). The facade re-exports
(`pub use oxvelte_parser as parser; pub use oxvelte_parser::ast;`) meant
zero consumer rewrites in this phase; the 68 linter files switch to real
crate paths in phase 6. Phase 4: `crates/oxvelte_compiler` created
(`compiler.rs` → `lib.rs`; `typescript_conditions` widened to `pub`;
`OXVELTE_REPO_ROOT = { value = ".", relative = true }` added to
`.cargo/config.toml` and all four `include_str!` sidecar embeds
re-anchored on it — verified working by the `valid_compile` suite
running the embedded bridge end-to-end). Phase 5: `crates/oxvelte_tsrs`
created (`tsrs/` moved; internal `crate::tsrs::` → `crate::`,
`crate::transport::` → `oxvelte_transport::`; `serde_json` dep added —
used via full paths). The root feature is now
`tsrs-inference = ["dep:oxvelte_tsrs"]`, and a feature-off root build has
zero `oxvelte_tsrs` in both the compile output and the dependency tree —
the crate genuinely does not exist without the feature (note:
`cargo check --workspace` still checks it, since `--workspace` means all
members; the gating claim covers builds of the `oxvelte` package).
Full suite green every phase (447 root + 2 compiler + 199 parser + 1
transport + 12 tsrs lib tests = the same 661). Phase 6:
`crates/oxvelte_linter` created (`linter/` + `compiler_ignore.rs` +
`config.rs` — moved in, not kept at root; see amended decision 6 — +
`build.rs` + `resources/`); `.gitignore` and `.github/release-files.txt`
updated; root `tsrs-inference` now forwards to
`oxvelte_linter/tsrs-inference`; the facade is complete
(`linter`, `config`, `compiler`, `parser`, `ast`, `transport`, `tsrs`
all re-exported). Full suite green every phase (275 root + 2 compiler +
172 linter + 199 parser + 1 transport + 12 tsrs lib tests = the same
661). Phases 7–8 not yet implemented. This doc is the authoritative
plan for the reorganization; update it if the crate boundaries change
during implementation.

### Preface

`oxvelte` is today a modular monolith: one Cargo package compiling ~50K lines
of `src/` into a single ~12 MiB binary. The proposal: split the crate into a
Cargo workspace of small, private (never-published) crates — a
component-based monolith — following the precedent set by `oxc` (`oxc_parser`,
`oxc_span`, …) and `tsrs` (`tsrs_compiler`, `tsrs_api`, …), whose `crates/`
directories hold 40+ unpublished internal crates.

Motivations, in priority order:

1. **Compile-time efficiency, especially on Windows.** Per-crate
   fingerprints mean unchanged components are never recompiled; cold/CI
   builds parallelize across crates; and feature-combination builds stop
   recompiling the entire 50K-line crate (today, building with and without
   `tsrs-inference` compiles everything twice — a large part of `target/`
   bloat, 13 GiB on the dev machine at time of writing).
2. **Enforced boundaries.** The module architecture documented in
   `docs/review/KIMI.md` becomes compiler-checked: a rule file that reaches
   into `compiler` internals simply fails to build.
3. **A home for upcoming transport work.** The planned VS Code extension
   channel (stdio + MessagePack + JSON) lands in an `oxvelte_transport`
   crate that already exists, instead of requiring a mid-project extraction.

Explicit non-goals:

- **No change to the shipped artifact.** Internal crates compile to rlibs
  and are statically linked into the same single `oxvelte` binary. We are
  deliberately **not** building per-component dylib/cdylib/DLL files: Rust
  has no stable ABI, dynamic loading would require `extern "C"` boundaries
  and runtime `dlopen`, would defeat cross-crate inlining and LTO, and is
  not what `oxc`/`tsrs` do with their internal crates either.
- **No binary-size or runtime-performance change expected.** The release
  profile (`lto = true`, `codegen-units = 1`) already optimizes across
  module boundaries today and will optimize across crate boundaries
  identically after the split.
- **No logic rewrites.** This is file moves, new `Cargo.toml` files, import
  path rewrites, and a small number of visibility widenings — detailed
  below. Rule logic, parser logic, and protocol logic are untouched.

### Measured baseline (dev machine, warm dependency cache)

| Metric | Value |
|---|---|
| Total `src/` | 50,148 lines |
| `src/linter/` | 27,326 lines (rules alone: 25,600 across 89 rule files) |
| `src/parser/` | 18,722 lines (`template.rs` 9,883; `serialize.rs` 5,307) |
| `src/tsrs/` | 2,616 lines; `src/transport/` 259 lines |
| Cold `cargo check --lib` (both features) | 12.2s |
| Cold `cargo build --lib` (dev profile) | 27.9s |
| Rebuild after editing one rule file | ~1s (incremental + 32 CGUs already work well locally) |

Local single-file iteration is already fast; the wins this plan buys are
cold-build parallelism, per-component fingerprints, and feature-combo
artifact reuse — all of which matter most on Windows and in CI, where every
rustc invocation and filesystem scan is substantially pricier.

### Current coupling map (grep over all `crate::` references)

| Component | Depends on (internal) | Notes |
|---|---|---|
| `ast.rs` | nothing (oxc, serde) | used by parser (6×) and linter (113×, 68 files) |
| `transport/` | nothing — pure std | consumed today only by `tsrs/` |
| `parser/` | `ast` only | cleanest extraction |
| `compiler.rs` | nothing internal (leaf) | hosts `byte_utf16`/`utf16_byte` |
| `compiler_ignore.rs` | `compiler`, `parser`, `linter`, `ast` | moves with the linter crate |
| `tsrs/` | `transport`, plus `compiler::byte_utf16` (6 call sites in `conditions.rs`) | feature-gated |
| `linter/` | `ast`, `parser`, `config`, `compiler`, `compiler_ignore`, `tsrs` (cfg-gated) | rules reach `Rule`, `LintContext`, `LintDiagnostic`, `RuleConfig`, `Linter`, `walk_template_nodes` from `linter/mod.rs` |
| `config.rs` | `linter::RuleConfig` (one type) | stays in root — root sits above linter, no cycle |
| `watch.rs` | `linter::rules::no_conflicting_module_names::MODULE_EXTENSIONS` | stays in root |
| `custom_rules.rs` | `linter` | stays in root behind `custom-rules` |

The graph is a clean DAG; the only back-edge is `config.rs → linter`, which
is fine because `config.rs` remains in the root package.

## Target layout

Top-level `crates/` directory — the tsrs/oxc convention. (A nested
`src/crates/` was considered and rejected: `src/` is the root package's own
source tree, and nesting packages inside it confuses Cargo's package
auto-discovery and rust-analyzer for no benefit.)

```
oxvelte-rs/
├── Cargo.toml                  # workspace root + root package (thin lib + bin)
├── src/                        # main.rs, lib.rs, config.rs, watch.rs,
│                               # custom_rules.rs
└── crates/
    ├── oxvelte_transport/      # src/transport/** + span helpers (from compiler.rs)
    ├── oxvelte_parser/         # src/parser/** + ast.rs as `pub mod ast`
    ├── oxvelte_compiler/       # src/compiler.rs
    ├── oxvelte_tsrs/           # src/tsrs/**
    └── oxvelte_linter/         # src/linter/** (mod.rs + rules/ + indent/)
                                #   + src/compiler_ignore.rs + build.rs + resources/
```

Dependency graph (strictly one-directional):

```
oxvelte_transport  (leaf, std-only)
oxvelte_parser     → oxc, oxc_diagnostics, serde, serde_json, selectors, cssparser
oxvelte_compiler   → oxvelte_transport, oxc, serde, serde_json
oxvelte_tsrs       → oxvelte_transport, oxc, serde, thiserror [optional]
oxvelte_linter     → oxvelte_parser, oxvelte_compiler, oxvelte_transport,
                     oxvelte_tsrs (optional), oxc, rustc-hash, regex,
                     serde, serde_json, selectors
oxvelte (root)     → all of the above + clap, rayon, atty, notify,
                     boa_engine (optional)
```

(`thiserror` is used only in `tsrs/mod.rs` and moves to `oxvelte_tsrs`;
`rayon` is used only in `main.rs`, `atty` only in `main.rs`, `notify` only
in `watch.rs` — all stay on the root. `miette` is declared today but has
zero use sites in `src/` or `tests/` — see the sanity-check section. External
allocation per component is from grep over `use` statements; exact per-crate
`[dependencies]` are settled by the compiler during each extraction phase.)

### Crate manifest conventions

- The root `Cargo.toml` is the **only** place `[workspace]` appears:
  ```toml
  [workspace]
  members = [".", "crates/oxvelte_*"]
  resolver = "2"
  ```
  `resolver = "2"` is explicit (sanity-check finding 7). Sub-crates never
  declare their own `[workspace]` table. Because the workspace root is the
  repo root, every member build (even a crate built individually) inherits
  the root `.cargo/config.toml` — env pins, fixture tags, and aliases keep
  working unchanged. (Implementation note, learned in phase 1: Cargo
  **rejects a member glob with zero matches**, so `members` starts as
  `["."]` and the `crates/oxvelte_*` glob is added in phase 2 alongside
  the first crate.)
- Package names and directory names use **underscores**, matching the
  oxc/tsrs precedent (`crates/oxc_parser` → package `oxc_parser`,
  `tsrs_compiler`, …): `oxvelte_transport`, `oxvelte_parser`,
  `oxvelte_compiler`, `oxvelte_tsrs`, `oxvelte_linter`. Feature names keep
  their existing hyphens (`tsrs-inference`, `custom-rules`).
- **Workspace inheritance everywhere.** Shared metadata and every shared
  dependency version are defined once in the root `Cargo.toml`:
  ```toml
  [workspace.package]
  version = "0.4.0"
  edition = "2021"
  license = "MIT"
  repository = "https://github.com/billpeet/oxvelte"
  homepage = "https://github.com/billpeet/oxvelte"

  [workspace.dependencies]
  oxc = { version = "=0.156.0", features = ["full", "serialize"] }  # keep the exact `=` pin
  oxc_diagnostics = "=0.156.0"
  serde = { version = "1", features = ["derive"] }
  # …serde_json, selectors, cssparser, regex, rustc-hash, rayon, clap, …
  ```
  and each sub-crate inherits:
  ```toml
  [package]
  name = "oxvelte_linter"
  version.workspace = true
  edition.workspace = true
  license.workspace = true
  repository.workspace = true
  homepage.workspace = true
  publish = false
  workspace = "../.."            # explicit back-pointer to the workspace root

  [dependencies]
  oxc = { workspace = true }
  oxc_diagnostics = { workspace = true }
  ```
  Version bumps then happen in exactly one place. **Caution:** the `oxc` /
  `oxc_diagnostics` pins must keep their exact `=0.156.0` form in
  `[workspace.dependencies]` — parity discipline depends on it.
  (`workspace = "../.."` is technically redundant with the `members` glob —
  membership is already bidirectional — but we declare it explicitly so the
  linkage survives future `members` edits. It changes no path resolution:
  `CARGO_MANIFEST_DIR` still points at each sub-crate's own directory.)
- **The root package inherits its version too** (`version.workspace =
  true`): the single literal lives in `[workspace.package]` and
  `scripts/release.sh` is updated to read/write that section specifically
  (sanity-check finding 6). `scripts/stage-npm.mjs` needs no change — its
  quote-requiring regex finds the workspace literal either way.
- Internal crates are `publish = false`; path dependencies carry no
  `version` field. (Verified: nothing in the repo runs
  `cargo publish`/`cargo package`; distribution is GitHub-release binaries,
  staged npm packages, and a source tarball driven by
  `.github/release-files.txt` — all of which tolerate path-only workspace
  dependencies.)

## Key decisions

1. **`transport/` is its own crate — `oxvelte_transport` — not folded into
   tsrs.** It is the defined location for how oxvelte communicates with
   other processes: tsrs MessagePack framing today, the planned VS Code
   extension channel (stdio + MessagePack + JSON) next. It also *cannot*
   remain a root-crate module once `tsrs/` is a crate: the root depends on
   `oxvelte_tsrs` (optionally), and Cargo forbids the reverse edge
   `oxvelte_tsrs → root` that `crate::transport` would become.
2. **`ast.rs` folds into `oxvelte_parser` as `pub mod ast`** rather than
   becoming a 500-line micro-crate. The AST is the parser's data model;
   linter's 113 references become `use oxvelte_parser::ast::*`.
3. **`byte_utf16` / `utf16_byte` move from `compiler.rs` to
   `oxvelte_transport`.** See the dedicated section below.
4. **`rules/` is NOT a separate crate.** All 89 rules consume `Rule`,
   `LintContext`, `LintDiagnostic`, `RuleConfig`, `Linter`, and
   `walk_template_nodes` from `linter/mod.rs`; splitting rules from the
   linter core means extracting a shared-types crate for marginal gain.
   One `oxvelte_linter` crate already isolates 27K lines.
   `rules/indent/` stays an ordinary submodule. Revisit only if a future
   `cargo build --timings` shows the linter crate dominating cold builds.
5. **`compiler_ignore.rs` moves with `oxvelte_linter`** (it depends on
   parser+linter); `compiler.rs` alone becomes `oxvelte_compiler`. The four
   `include_str!("../scripts/compiler-runtime/*.cjs")` embeds in
   `compiler.rs:155-160` must be re-anchored — see the sanity-check section.
6. **`watch.rs`, `custom_rules.rs`, `main.rs` stay in the root package;
   `config.rs` moves INTO `oxvelte_linter`** (amended 02NOV2026 during
   phase 6). The original decision kept `config.rs` in the root because
   it imports `linter::RuleConfig` — but phase 6 surfaced the reverse
   edge this analysis had under-weighted: `linter/mod.rs` and two rule
   files reference `config::OxvelteConfig` 12 times, including in the
   `Linter` public API (`add_enabled_rules`, the `LintContext::Project`
   variant). linter→root and root→linter is a true dependency cycle, so
   `config.rs` became `oxvelte_linter::config` and the facade re-exports
   it (`pub use oxvelte_linter::config;`), keeping `oxvelte::config`
   working for `main.rs`, the fixture tests, and `tests/`.
7. **`build.rs` and `resources/` move to `oxvelte_linter`.** The generated
   `known_css_properties.json` is consumed by
   `rules/no_unknown_style_directive_property.rs` via
   `include_str!(concat!(env!("OUT_DIR"), …))`, and build scripts are
   per-crate — so the build script, its `[build-dependencies]` (flate2,
   tar, ureq, zip, serde_json), and the cached `resources/` tree move
   together. Three non-code updates ride along: the `.gitignore` pattern
   `/resources/css/known_css_properties.json` is root-anchored and must
   move to `/crates/oxvelte_linter/resources/css/known_css_properties.json`;
   `.github/release-files.txt` replaces the root `build.rs` entry with
   `crates/` (see sanity-check section); and `.cargo/config.toml` commit
   pins are untouched. The cached file keeps release builds offline as
   today (the release-tarball smoke test builds with network, so it would
   fetch if the cache were absent — the offline check in phase 6 is
   therefore manual, not CI-enforced).
8. **The root `oxvelte` lib is a thin facade only for the duration of the
   migration.** It re-exports the crate roots under their historical module
   paths so `tests/`, `src/main.rs`, and the fixture-driven unit test
   modules in `lib.rs` compile unchanged while phases land. **Phase 8
   removes the facade** and switches all consumers to the real crate paths —
   see that phase for the full recipe, including the replacement for the
   `[dev-dependencies]` self-dependency.

## The `byte_utf16` / `utf16_byte` move — full scope

What they are (`src/compiler.rs:278-289`): two pure, stateless ~10-line
functions converting between Rust/oxc **byte offsets** and **UTF-16
code-unit indices**. They exist only where data crosses a process boundary:
the Svelte compiler's warnings, the tsrs wire protocol, and ESLint's column
conventions all speak UTF-16; oxc speaks bytes. The planned VS Code channel
will need them too — LSP `Position.character` is UTF-16 by spec.

Why they move: once `tsrs/` is a crate, `crate::compiler::byte_utf16` would
otherwise force `oxvelte_tsrs → oxvelte_compiler`, a semantic inversion
(the tsrs backend is an *alternative* to the JS sidecar, not a client of
it) that drags process-spawning sidecar machinery into tsrs's dependency
graph for two pure functions. Duplicating them is rejected on correctness
grounds: the `tsrs_conditions` suite requires the js-sidecar and tsrs legs
to agree **byte-exactly**, so the span-conversion logic must have exactly
one definition.

Steps:

1. Cut `byte_utf16` and `utf16_byte` (`src/compiler.rs:278-289`) into
   `crates/oxvelte_transport/src/span.rs` as `pub fn`, exported from the
   transport crate root (`pub mod span;`). New home is thematically
   consistent: span-unit encoding is part of every wire protocol oxvelte
   speaks.
2. Move their unit test (`utf16_positions_map_to_utf8_boundaries`,
   `src/compiler.rs:416-420`) with them.
3. Update the three consumer files — import lines only, call sites
   unchanged:
   - `src/compiler.rs` (`:243-244`, `:260`) →
     `use oxvelte_transport::span::{byte_utf16, utf16_byte};`
   - `src/tsrs/conditions.rs:18` (6 call sites) →
     `use oxvelte_transport::span::byte_utf16;`
   - `src/linter/rules/no_unnecessary_condition.rs:3` (3 call sites) →
     `use oxvelte_transport::span::{byte_utf16, utf16_byte};`
4. Both functions are currently `pub(crate)`; they become `pub` in the new
   crate. Nothing outside `src/` references them (verified: no hits in
   `tests/` or `scripts/`).

No logic changes — the function bodies move verbatim.

## Feature plumbing and the dev/test trick (phases 1–7)

- `oxvelte_linter` declares:
  `tsrs-inference = ["dep:oxvelte_tsrs"]` — this is where the cfg-gated
  consumer (`rules/no_unnecessary_condition.rs`) lives, so this is where
  the gate must exist. With the feature off, Cargo never builds or
  fingerprints `oxvelte_tsrs` at all.
- The root package forwards:
  ```toml
  [features]
  default = []
  custom-rules = ["dep:boa_engine"]                       # unchanged; custom_rules.rs stays in root
  tsrs-inference = ["dep:oxvelte_tsrs", "oxvelte_linter/tsrs-inference"]
  ```
  (`dep:oxvelte_tsrs` on the root exists solely so the facade can
  re-export `oxvelte::tsrs`; it is dropped in phase 8.)
- `release.yml` keeps building with `--features custom-rules,tsrs-inference`
  verbatim; CI (`ci.yml`) is untouched.
- **The `[dev-dependencies]` self-dependency stays verbatim through phase
  7:**
  ```toml
  [dev-dependencies]
  oxvelte = { path = ".", features = ["tsrs-inference", "custom-rules"] }
  ```
  It names the root *package* `oxvelte` — not any module — to force both
  features on for every `cargo test` and for rust-analyzer. While the
  facade exists, every path the tests use (`oxvelte::parser`,
  `oxvelte::tsrs`, …) resolves through it and the trick works unchanged.
  Phase 8 replaces it with direct per-crate dev-dependencies.

## Root facade (phases 1–7 only)

`src/lib.rs` keeps its fixture-driven test modules and replaces the moved
`pub mod` declarations with re-exports:

```rust
pub use oxvelte_parser as parser;
pub use oxvelte_parser::ast;            // preserves oxvelte::ast
pub use oxvelte_compiler as compiler;
pub use oxvelte_linter as linter;

pub mod config;                         // unchanged root modules
pub mod watch;
#[cfg(feature = "custom-rules")]
pub mod custom_rules;

#[cfg(feature = "tsrs-inference")]
pub use oxvelte_transport as transport; // cfg kept for API parity with today
#[cfg(feature = "tsrs-inference")]
pub use oxvelte_tsrs as tsrs;
```

Every existing `oxvelte::parser`, `oxvelte::linter`, `oxvelte::ast`, …
path used by `tests/` (17 use sites across the suites) keeps working while
the facade exists. Phase 8 removes it.

## What changes, besides moving files — the complete honest list

1. The two-function `byte_utf16`/`utf16_byte` relocation above (pure move).
2. Import-path rewrites — the bulk of the diff, concentrated in
   `oxvelte_linter`: `crate::ast::` → `oxvelte_parser::ast::` (68 files),
   `crate::parser::` → `oxvelte_parser::`, `crate::compiler::` →
   `oxvelte_compiler::`, `crate::tsrs::` → `oxvelte_tsrs::`,
   `crate::transport::` → `oxvelte_transport::`.
3. Visibility widenings where an item crosses a new crate boundary:
   - `parser::scanner`: `pub(crate)` → `#[doc(hidden)] pub`
     (used by `compiler_ignore.rs`).
   - `byte_utf16`/`utf16_byte`: `pub(crate)` → `pub`.
   - `compiler::typescript_conditions` (`compiler.rs:313`):
     `pub(crate)` → `pub` (called from
     `rules/no_unnecessary_condition.rs:116`).
   - Any additional `pub(crate)` items the compiler flags during
     extraction; each one is widened, never refactored.
4. `Cargo.toml` work: new per-crate manifests, `[workspace]` +
   `[workspace.package]` in the root, feature forwarding.
5. `build.rs` / `resources/` relocation to `oxvelte_linter`, adjusting
   paths inside `build.rs` that are relative to the crate manifest dir.
6. Phase 8 only: import-path updates in `tests/` and the root `src/`
   files when the facade is removed (still import lines, not logic).

There is **no** logic change anywhere — not in rules, parser, tsrs, or the
compiler bridge. If an extraction step ever seems to require a logic
change, that is the signal to stop and re-examine the crate boundary, not
to make the change.

## Sanity-check findings (02NOV2026, pre-implementation audit)

Verified by reading the files being moved and the release machinery. These
are the gotchas a naive `git mv` would hit; all are accounted for above or
in the phase list.

1. **`compiler.rs:155-160` embeds four JS files with source-relative
   `include_str!("../scripts/compiler-runtime/*.cjs")`** (`bridge.cjs`,
   `typescript-service.cjs`, `typescript-conditions.cjs`,
   `vendor/sourcemap-codec.cjs`). After the move to
   `crates/oxvelte_compiler/src/compiler.rs`, `../scripts` no longer
   resolves. Fix: a dedicated repo-root anchor in `.cargo/config.toml`,
   using the same `relative = true` mechanism the project already relies on
   for `OXVELTE_COMPILER_RUNTIME`:
   ```toml
   # .cargo/config.toml
   OXVELTE_REPO_ROOT = { value = ".", relative = true }
   ```
   ```rust
   // crates/oxvelte_compiler/src/compiler.rs
   include_str!(concat!(env!("OXVELTE_REPO_ROOT"),
       "/scripts/compiler-runtime/bridge.cjs"))
   ```
   `relative = true` makes Cargo resolve the value against the directory
   containing `.cargo/` on whatever machine builds — an absolute,
   machine-correct path at compile time, with no `../../` chains and
   nothing hardcoded. Two rejected alternatives, for the record: a runtime
   `find_workspace_root()` helper **cannot** work here — `include_str!` is
   a compile-time macro — and reusing `OXVELTE_COMPILER_RUNTIME` for the
   embed would couple compile-time content to an overridable runtime
   variable (the version matrix and CI override it per profile; an
   override at build time would change which files get embedded). The
   scripts themselves stay at the repo root (they are a sidecar project
   with their own JS test suites; `release.yml` sets
   `OXVELTE_COMPILER_RUNTIME` to them). Note `build.rs` needs **no** such
   anchor: `resources/` moves into `crates/oxvelte_linter/` with it, so
   `manifest_dir.join("resources/css/known_css_properties.json")` remains
   correct verbatim.
2. **`.github/release-files.txt` must be updated.** The source-release
   workflow packs exactly the listed paths and smoke-tests that the tarball
   compiles with `cargo build --release --locked` — anything a build needs
   MUST be named there. After the reorg: add `crates/`, drop the root
   `build.rs` entry (it moves into `crates/oxvelte_linter/`), keep
   `scripts/compiler-runtime/*.cjs` (still embedded, per finding 1). The
   four `.cjs` paths in the list stay valid.
3. **The `.gitignore` entry for the CSS cache is root-anchored**:
   `/resources/css/known_css_properties.json`. It must move to
   `/crates/oxvelte_linter/resources/css/known_css_properties.json` or the
   regenerated cache at the new location would become an untracked-file
   hazard. (`resources/` is gitignored and regenerated — it is correctly
   absent from `release-files.txt`; the tarball smoke test fetches over the
   network. The manual offline check in phase 6 exists because CI would not
   catch a broken cache path.)
4. **`Cargo.lock` churn vs `--locked` release builds.** New workspace
   members change the lockfile; `release.yml` and the tarball smoke test
   build with `--locked`, so the updated `Cargo.lock` must be committed in
   the same phase that changes any manifest.
5. **`miette` is a dead dependency** (`Cargo.toml:39`, with the expensive
   `fancy` feature): zero `miette` use sites in `src/` or `tests/`. Out of
   scope for this reorg — flag to the maintainer; drop separately if
   unwanted.
6. **Release tooling reads the root `Cargo.toml` version line directly.**
   `scripts/release.sh:12` reads `grep '^version' Cargo.toml | head -1`
   (first line starting with `version` — an inherited
   `version.workspace = true` line would break the read) and `:37` rewrites
   `^version = "…"` on every matching line. `scripts/stage-npm.mjs:55`
   uses `/^version\s*=\s*"([^"]+)"/m` (requires a quoted value, so
   inheritance lines never match). Resolution: the version literal moves
   **only** to `[workspace.package]` (single source of truth; root and
   sub-crates all inherit), and `release.sh` gets a section-scoped read
   and write. Both use **perl only** (decided: no sed, no awk in
   `release.sh`) — perl behaves byte-identically on GNU and BSD with no
   `-i` flag divergence, reads more clearly than a sed address range, and
   ships with both macOS and Ubuntu. `release.sh` gains two helpers:
   ```sh
   get_version() {
       perl -ne '
         if (/^\[/) { $found = 0; }
         if (/^\[workspace\.package\]$/) { $found = 1; }
         if ($found && /^version\s*=\s*"([^"]+)"/) { print $1; exit; }
       ' Cargo.toml
   }

   set_version() {
       perl -i -pe '
         if (/^\[/) { $found = 0; }
         if (/^\[workspace\.package\]$/) { $found = 1; }
         if ($found && !$done && s/^version = "\Q'"${CURRENT}"'\E"/version = "'"${NEW}"'"/) { $done = 1; }
       ' Cargo.toml
   }
   ```
   and the write is followed by a verification gate (re-read and compare;
   abort loudly on mismatch):
   ```sh
   CURRENT=$(get_version)
   # …validate NEW, then:
   set_version
   if [ "$(get_version)" != "${NEW}" ]; then
       echo "release.sh: Cargo.toml version check failed after update (expected ${NEW}, got $(get_version))" >&2
       exit 1
   fi
   ```
   Requirements verified by test on a simulated future manifest (macOS
   perl): the read finds `version` at any key position within the
   `[workspace.package]` section (tested as the 4th key); the write
   changes exactly that one line and nothing else; the `/^\[/
   { $found = 0 }` section reset is **mandatory** in both helpers —
   without it a missing/moved version key causes a silent read of, or
   write into, a later section; `\Q…\E` quotes the old version so regex
   metacharacters can never widen the match; `perl -i` (no suffix) leaves
   no `.bak` file to clean up; a wrong `CURRENT` produces a no-op write
   (diff-clean); and against today's pre-workspace manifest both helpers
   fail safe (empty read, untouched file). `stage-npm.mjs` is unchanged:
   its regex finds the `[workspace.package]` literal (confirmed: reads
   `0.5.0` after a simulated bump).
7. **Set `resolver = "2"` explicitly in `[workspace]`.** Adding a
   `[workspace]` table without a resolver key makes Cargo fall back to
   resolver 1 semantics (with a warning), which unify features across
   build- and dev-dependency edges — the wrong default for an edition-2021
   project, and directly hostile to this plan's feature-gating goals
   (a dev-only feature could leak into a release build of a dependency).
   Resolver 2 matches the current single-package behavior most closely.
8. **`build.rs:165`'s user-agent is `concat!("oxvelte/",
   env!("CARGO_PKG_VERSION"), " build.rs")`.** After the move,
   `CARGO_PKG_VERSION` is `oxvelte_linter`'s version — which, because
   sub-crates use `version.workspace = true`, stays `0.4.0` and the
   user-agent string is unchanged. (This is also why the sub-crates
   inherit the workspace version rather than using `0.0.0`.)

Non-issues, checked and cleared: no `#[path = …]` attributes in `src/`;
`tests/support/*.rs` `#[path]` includes are relative to the test files and
unaffected; `scripts/launcher.rs` is pure std with no dependency on the
lib; fixture paths in `lib.rs`/tests are CWD-relative (`fixtures/…`) and
`cargo test` runs from the workspace root, so they are unaffected;
`linter/mod.rs` has zero direct `tsrs` references (the only tsrs consumer
in the linter is `rules/no_unnecessary_condition.rs`, already cfg-gated);
a full `env!`/`CARGO_PKG_*` sweep of `src/` found no compile-time path
anchors besides the finding-1 embeds and the `OUT_DIR` read, so
`OXVELTE_REPO_ROOT` is the only new anchor needed; Rust never reads
`OXVELTE_COMPILER_RUNTIME` (only the spawned JS sidecar does, at runtime),
so the move cannot disturb runtime resolution; the `oxc`/`oxc_diagnostics`
exact pins live once in `[workspace.dependencies]` and every crate
inherits the identical pin (Cargo unifies them into a single build).

## Phases

Each phase is a separately landable unit and ends green on:
`cargo fmt --check`, `cargo clippy --all-targets` (0 warnings), and the full
Rust suite — `cargo test --lib`, `--test upstream_parity`,
`--test tsrs_conditions`, `--test tsrs_wire_protocol`,
`--test parser_lint_contract`, `--test upstream_adapter`,
`--test jobsys_regressions`.

1. **Workspace-ify.** Root `Cargo.toml` gains `[workspace] members =
   ["."], resolver = "2"` (the `crates/oxvelte_*` glob is added in phase
   2 — Cargo rejects an empty glob), `[workspace.package]`, and
   `[workspace.dependencies]` (all shared dependency versions move here;
   the root package's own `[dependencies]` inherit with
   `{ workspace = true }`; the `oxc`/`oxc_diagnostics` pins keep their
   exact `=0.156.0` form; the version literal moves to
   `[workspace.package]` per finding 6, with `release.sh` gaining the
   `get_version()`/`set_version()` perl helpers and post-write
   verification gate there). No code moves; verifies the build/test/CI
   harness tolerates workspace mode, that inheritance resolves identically
   to the literal versions, and that the release tooling still reads and
   bumps the version (run the finding-6 helpers by hand, plus the
   `stage-npm.mjs` regex). Commit the regenerated `Cargo.lock`
   with every phase that touches a manifest (release builds use
   `--locked`).
2. **`oxvelte_transport`.** `git mv src/transport
   crates/oxvelte_transport/src`; move the span helpers in (section
   above); root `Cargo.toml` gains the path dependency; three consumer
   files update imports.
3. **`oxvelte_parser`.** Move `src/parser/` + `src/ast.rs` (as
   `oxvelte_parser::ast`); widen `scanner` visibility; rewrite
   `crate::parser`/`crate::ast` imports across `src/`. Largest single
   compile-time win (19K lines off the root fingerprint).
4. **`oxvelte_compiler`.** Move `src/compiler.rs`; add `OXVELTE_REPO_ROOT`
   to `.cargo/config.toml` and re-anchor the four `include_str!` embeds on
   it (sanity-check finding 1); widen `typescript_conditions` to `pub`;
   update imports.
5. **`oxvelte_tsrs`.** Move `src/tsrs/` behind `dep:oxvelte_tsrs` under
   `tsrs-inference` (temporarily gated at the root until phase 6 moves the
   consumer). Verify a build without the feature never compiles the
   crate; verify `--features tsrs-inference` builds and the
   `tsrs_conditions`/`tsrs_wire_protocol` suites are unchanged.
6. **`oxvelte_linter`.** Move `src/linter/` + `src/compiler_ignore.rs` +
   `src/config.rs` (see amended decision 6) + `build.rs` + `resources/`;
   move the `.gitignore` cache pattern and update
   `.github/release-files.txt` (sanity-check findings 2–3); move the
   feature gates onto this crate and add root forwarding; rewrite the
   `crate::` imports in the 89 rule files. Root `lib.rs` becomes the
   facade. Run the offline-cache check manually (cache present, no
   network fetch) before landing. Implementation notes (02NOV2026): the
   126 grouped `use crate::{…}` imports in rule **test modules**
   (`crate::{linter::Linter, parser}` style, written against the
   pre-split layout) are bridged by three cfg(test)-gated aliases at the
   linter crate root (`use crate as linter; use oxvelte_parser as
   parser; use oxvelte_parser::ast;`) — invisible in the public API,
   removed in phase 8's cleanup; `npm_range_may_include_major` widened
   to `pub` for the root fixture tests; `cssparser`/`precomputed-hash`
   deps added (full-path usage, invisible to the `use`-grep).
7. **Measure and document.** `cargo build --timings` before/after on a
   clean target — including on a Windows machine, the motivating case —
   plus `du -sh target` across feature combinations (with/without
   `tsrs-inference`). Record results in a dated follow-up doc here and
   update the layout table in `docs/review/KIMI.md`.
8. **Drop the facade; point every consumer at the real crate paths.** The
   migration is not considered finished until the historical module paths
   are gone.
   - Remove the `pub use … as …` re-exports from `src/lib.rs`.
   - Root `src/` consumers switch to direct crate paths:
     `main.rs` (`oxvelte::{linter, parser}` → `oxvelte_linter` /
     `oxvelte_parser`), `lib.rs` fixture-test modules (`crate::parser` →
     `oxvelte_parser`, `crate::linter` → `oxvelte_linter`,
     `crate::config` stays), `watch.rs`, `custom_rules.rs`, `config.rs`.
   - `tests/` (17 `oxvelte::…` use sites across the suites +
     `tests/support/`) switch to `oxvelte_parser::`, `oxvelte_linter::`,
     `oxvelte_tsrs::`, `oxvelte_transport::`, `oxvelte_compiler::` as
     needed.
   - **Replace the self-dependency trick** in `[dev-dependencies]`. Today
     `oxvelte = { path = ".", features = ["tsrs-inference",
     "custom-rules"] }` exists to force features on for `cargo test` and
     rust-analyzer via the facade. With the facade gone, the test targets
     depend on the component crates directly:
     ```toml
     [dev-dependencies]
     oxvelte_linter   = { path = "crates/oxvelte_linter", features = ["tsrs-inference"] }
     oxvelte_tsrs     = { path = "crates/oxvelte_tsrs" }
     oxvelte_parser   = { path = "crates/oxvelte_parser" }
     oxvelte_transport = { path = "crates/oxvelte_transport" }
     oxvelte_compiler = { path = "crates/oxvelte_compiler" }
     ```
     Feature unification turns `tsrs-inference` on for the whole dev/test
     build graph, preserving today's behavior. **Verification point:**
     confirm no test or fixture path relies on the root package's own
     `custom-rules` feature before deleting the self-dep; if any does,
     keep a minimal self-dep (or gate that consumer) rather than silently
     changing `cargo test` behavior.
   - The root package keeps `custom-rules` (it owns `custom_rules.rs`) and
     keeps forwarding `tsrs-inference = ["oxvelte_linter/tsrs-inference"]`
     so the **release binary** still builds with the same
     `--features custom-rules,tsrs-inference` invocation; the
     `dep:oxvelte_tsrs` edge on the root is removed with the facade.
   - Final sweep: `grep -rn 'oxvelte::' tests/ src/` returns no stale
     facade paths.

## Risks

- **Visibility creep.** Each `pub(crate)` → `pub` widening technically
  grows the public API surface of the internal crates. Mitigation: the
  crates are `publish = false`; widen with `#[doc(hidden)]` where the item
  is internal machinery (e.g. `scanner`).
- **Feature unification surprises.** With the dev-dependency arrangement,
  the linter is built with `tsrs-inference` in dev/test; forwarding must
  be wired so a release build without features stays truly minimal.
  Phases 5 and 8 verify this explicitly.
- **`build.rs` path regressions.** The CSS fetch script uses
  manifest-relative paths; phase 6 runs the offline-cache path explicitly
  (release-style build with the cached `resources/` file) before landing.

## Success criteria

- Identical CLI behavior and identical test results across all suites.
- Release binary remains a single statically-linked executable of
  comparable size; `release.yml` unchanged.
- Clean-build wall time reduced on Windows CI; `target/` no longer
  duplicates the full 50K-line compile per feature combination.
- No rule/parser/protocol logic diffs — only moves, imports, visibility,
  and manifests.
- After phase 8: no facade re-exports remain; `tests/` and `src/` import
  the `oxvelte_*` crates directly.
