# Plan: native Svelte 5 compilation via rsvelte (forked), replacing the JS sidecar compile path

**Status:** researching 02NOV2026 — fork created (`github.com/btbutts/rsvelte`), oxc-unification
spike completed, integration not started
**Scope:** `github.com/btbutts/rsvelte` (fork of `baseballyama/rsvelte` @ `15cf0877`),
root `Cargo.toml`, new crate `crates/oxvelte_rsvelte/`, `crates/oxvelte_compiler/`,
`crates/oxvelte_linter/`, `src/config.rs` (settings schema), parity + integration tests
**Goal:** for Svelte 5 projects, lint components with the Rust-native rsvelte compiler instead of
spawning the JS `bridge.cjs` sidecar — opt-in and experimental, exactly the `tsrs-inference`
pattern (cargo feature **and** `oxvelte.config.json` setting, both defaulting off). The JS bridge
is retained, still embedded, and still authoritative for everything rsvelte cannot do.

## What does NOT change (settled, do not re-litigate)

- **`bridge.cjs` is not deleted.** It remains the compile path for Svelte 4 projects, for
  components with preprocessed styles (`scss`/`less`/`stylus`/`postcss`), for reading executable
  `svelte.config.js` `compilerOptions`, for the `warningFilter`/`onwarn` callbacks operation
  (already a separate bridge operation — untouched), and for the `typescript-conditions` JS
  fallback when tsrs routing declines. When the rsvelte path declines or the feature is off,
  behavior is byte-identical to today.
- **Source-map work stays as planned.** `rust_sourcemap_decode.md` is orthogonal and proceeds on
  its own track; a future phase noted below lets the two plans compound.
- **No downgrade of our oxc pin.** We keep `=0.156.0` (we use newer oxc features; retesting our
  own tree against a downgrade is rejected). Unification goes the other way: the fork upgrades
  rsvelte's oxc to 0.156.
- **No production ambitions.** rsvelte output (JS/CSS) is discarded; only its diagnostics are
  consumed. rsvelte is pre-1.0 — acceptable inside the experimental gate, same bucket as
  `tsrs-inference`.

## Findings: the rsvelte project (verified 02NOV2026)

`rsvelte` (`github.com/baseballyama/rsvelte`) is a Rust port of the official Svelte 5 compiler,
**targeting Svelte v5.57.1**, passing 100% of the in-scope official Svelte fixtures, MIT-licensed,
in active daily development (HEAD `15cf0877`, 2026-10-31). Its README names oxlint-style native
embedding as its reason to exist.

API fit via the facade crate `rsvelte` (verified in source):

- `Engine` is stateless, does **no filesystem access**, owns no caches/threads — matches our
  in-process lint model. One engine can be constructed once and shared.
- `engine.prepare(source, ComponentOptions)` → `component.compile(RuntimeTarget::Client)` →
  `RuntimeArtifact { javascript, css, diagnostics, runes }`. We discard `javascript`/`css`.
- `Diagnostic { severity, code, message, filename, span: Option<ByteRange> }` — **official Svelte
  warning codes and UTF-8 byte spans in the original source** (simpler than the bridge's UTF-16
  round-trip; feeds `Warning.span` directly).
- `CompileFailure` (parse/analysis error) maps onto our `kind: "error"` + single-warning path.
- `Engine::fingerprint().svelte_version` supplies our `compiler_version` string;
  `svelte_major` is 5 by construction on this path.
- `ComponentOptions` covers what we use: `custom_element` (our `_oxvelteCustomElement`), `runes`,
  `development`, `preserve_whitespace`, `disclose_version`, `css_mode`, `fixed_css_scope`.
- Their stated contract — "the facade always returns all diagnostics; embedders apply warning
  policy after compilation" — is exactly our architecture: warning policy (`warningFilter`/`onwarn`)
  already lives in the separate `callbacks` bridge operation, which stays.

Limitations (accepted, with mitigations below):

1. **No `generate: false`.** `compile()` always runs transform+codegen whose output we discard —
   wasted work per component versus the official compiler's analysis-only mode. Still likely beats
   a subprocess round-trip; measured in Phase 3. (Bypassing the facade for `rsvelte_core`'s
   analysis pass would avoid it but forfeits the stable API boundary — rejected for now.)
2. **No `experimental.async` option** (async is stable at Svelte 5.57). One existing bridge test
   (`identical_component_requests_refresh_config_and_imported_styles`) toggles it; that test is
   bridge-path-only and unaffected.
3. **Version drift by design.** rsvelte emulates 5.57.1 exactly; a project on another 5.x may see
   slightly different warnings than its installed compiler would emit, and `compiler_version`
   reports rsvelte's target, not the project's installed svelte. Acceptable inside the opt-in
   gate; quantified by the parity run in Phase 3.
4. **Pre-1.0 API.** Rev-pinned; the facade carries schema versions for cache identity.

## Dependency strategy: fork + oxc unification (spike-validated)

**crates.io is not an option yet** — `rsvelte`, `rsvelte_core`, `rsvelte_esrap`,
`rsvelte_projection` are all unpublished (verified 02NOV2026). Git dependencies with a `rev` pin
are required. Note: `tag = "0.3.17"` does **not** resolve — their tags are npm-package-scoped
(`@rsvelte/vite-plugin-svelte-native-win32-x64-msvc@0.3.17`); only the commit hash works.

**The oxc conflict and why the fork is required.** `rsvelte_core` pins `oxc_* = "0.149"` (63
requirements across its workspace); we pin `=0.156.0`. `[patch.crates-io]` cannot bridge a
semver-incompatible requirement, so a shared oxc tree is impossible without editing their
manifests — hence the fork `github.com/btbutts/rsvelte`. (Their own repo `[patch]`es oxc to the
v0.149.0 release commit; downstream consumers ignore dependency `[patch]` sections, so we would
otherwise inherit a full second oxc 0.149 tree — the dominant cost this plan avoids.)

**Spike (completed 02NOV2026, throwaway clone):** all 63 requirements rewritten to `=0.156.0`,
their `[patch]` block removed, `cargo check -p rsvelte`:

- All `oxc_parser` / `oxc_semantic` / `oxc_codegen` / `oxc_syntax` usage compiled untouched.
- **48 errors in 3 mechanical classes** — no redesigns, no semantic puzzles:

  | oxc change 0.149→0.156 | Sites | Fix shape |
  |---|---|---|
  | `StringLiteral.value`: `Atom` → `JSStr` (WTF-8; no `Display`, `as_str()` → `Option<&str>`) | ~27 | `display().to_string()` / `as_str().unwrap_or_default()` per site |
  | `StaticPropertyName` reshaped (lost `Display`/`to_string`) | ~9 | match / name accessor |
  | `CommentKind` gained `HtmlOpen`/`HtmlClose`; `Comment` lost `Copy` | ~10 | add match arms; clone |
  | `rsvelte_esrap` printer (same `JSStr` cause) | 2 | fixed during spike |

- Spike deliberately stopped short of forcing green: several `JSStr` conversions involve
  lone-surrogate stringification choices that must be made deliberately, and only rsvelte's own
  fixture suite can prove the port behavior-neutral. A misleading "it compiles" was rejected.

**Fork lifetime.** Upstream bumps oxc every 2–4 weeks and currently lags ~2 months; all their
bumps land as routine PRs. After our bump is validated, opening it as an upstream PR is the
preferred retirement path for the fork.

**Toolchain.** rsvelte uses edition 2024 / `rust-version = "1.97"`; our `rust-toolchain.toml`
tracks `stable`, so rustup handles this with no repo change.

## Integration design

### Placement and gating

- New crate **`crates/oxvelte_rsvelte/`** (`publish = false`, workspace-inherited metadata): the
  adapter. Wraps `rsvelte::Engine`, exposes
  `compile(source, filename, options) -> Result<CompileResult, String>` returning the existing
  `oxvelte_compiler::CompileResult` shape (via a shared types module or a conversion in
  `oxvelte_compiler` — decided in Phase 2; `CompileResult` may need to move or be re-exported).
- Dependency wiring mirrors `oxvelte_tsrs`: `rsvelte = { git = "https://github.com/btbutts/rsvelte",
  rev = "<fork bump commit>", optional = true }` on the adapter; cargo feature
  `rsvelte-compile = ["dep:rsvelte"]` (name finalized in Phase 1) on `oxvelte_compiler` or the
  adapter, forwarded by `oxvelte_linter` and the root crate, off by default. Feature-off builds
  never compile (and for git deps, never meaningfully touch) the rsvelte tree.
- Runtime gate: `settings.rsvelte` in `oxvelte.config.json` (boolean or object, schema decided in
  Phase 2), default off — the same dual-gate shape as `tsrs-inference` + `settings.tsrs`.

### Per-component routing (in `oxvelte_compiler::compile`)

With the feature compiled in AND `settings.rsvelte` enabled, route to rsvelte when **all** hold:

1. **Project is Svelte 5.** Chicken-and-egg today (`svelte_major` comes back from the bridge).
   Resolution: probe the nearest `node_modules/svelte/package.json` version once per project root
   (cached), falling back to the bridge when unresolvable. The config setting is the user's
   assertion; the probe is the guardrail against silently miscompiling a Svelte 4 project.
2. **No preprocessed styles.** Any `<style lang>` other than css → bridge (their preprocessors are
   user npm packages; rsvelte runs none).
3. **No executable config affecting compilation.** If `settings.compiler` carries
   `executableConfigPath`/`svelteConfig` with compile-relevant options (`compilerOptions`,
   `warningFilter`, `onwarn`), Phase 2 routes to the bridge; reading them requires executing JS.
   (`warningFilter`/`onwarn` alone could still route rsvelte for compilation + bridge for the
   existing `callbacks` operation — a Phase 3 refinement once the base path is proven.)

`lang="ts"` scripts do **not** block routing: rsvelte parses TypeScript natively through oxc,
which also eliminates the bridge's `ts.transpileModule` path for these components. Babel-parser
projects (`settings.compiler.parser === '@babel/eslint-parser'`) route to the bridge.

### Result mapping

`Diagnostic` → `Warning` directly (codes and byte spans pass through; `filtered: false`,
`report: None`); `CompileFailure` → `kind: "error"` with one warning; `strip_style_elements`,
`unused_ignores`, `ignore_items` empty (style stripping is a bridge/preprocessor concern);
`compiler_version` from `Engine::fingerprint().svelte_version`, `svelte_major: 5`. Everything
downstream — rules, ignore handling, the `callbacks` operation — is unchanged.

## Phases

Each phase ends green: `cargo test --workspace`, `cargo clippy --workspace --all-targets
--features tsrs-inference,custom-rules` (0 warnings), `cargo fmt --check` — plus, from Phase 2 on,
the same gates with the rsvelte feature enabled.

**Phase 0 — fork bump (repo: `btbutts/rsvelte`).**
Branch from `15cf0877`; rewrite the 63 oxc requirements to `=0.156.0`; drop the `[patch.crates-io]`
block; fix the 48 spike-catalogued errors deliberately (each `JSStr` conversion chosen per site);
validate with rsvelte's **own full gate**: `git submodule update --init --checkout --recursive`,
`pnpm install`, `pnpm run generate-fixtures`, `cargo test --release` — 100% fixture compliance on
oxc 0.156 is the definition of done. Tag/record the fork commit. Optionally open the bump as an
upstream PR.

**Phase 1 — adapter crate (this repo).**
`crates/oxvelte_rsvelte/` with the engine wrapper, `Diagnostic`→`CompileResult` mapping, unit
tests over a small fixed corpus (plain, runes, non-runes, TS script, parse error, CSS warning),
feature wiring through `oxvelte_compiler` → `oxvelte_linter` → root. Routing still hard-off
(adapter only callable in tests).

**Phase 2 — routing + settings.**
The routing rules above inside `oxvelte_compiler::compile`; `settings.rsvelte` schema in
`src/config.rs`; the svelte-version probe with caching; integration tests exercising both routes
and every decline reason (Svelte 4 probe, scss style, executable config, babel parser).

**Phase 3 — validation + metrics.**
Run the `upstream_parity` suite (1316 cases) with the rsvelte path forced on for eligible
components — acceptance: zero new gaps; investigate every diff as either an rsvelte bug (report
upstream) or a documented drift. Measure: lint wall-time rsvelte vs bridge on the demo/testbed
corpus (quantifies the codegen-waste question); clean-build time with the feature on vs off
(quantifies the added rsvelte tree on this Mac; Windows deferred as before). Update this file's
status; document the gate in `README.md`/config docs as experimental.

**Future (not scheduled):** with `rust_sourcemap_decode.md` landed, route preprocessed-style
components through JS-preprocess-only + rsvelte-compile + Rust remap, shrinking the bridge to
preprocessors and callbacks for Svelte 5. Evaluate `rsvelte_projection` as a replacement for
`oxvelte_tsrs`'s hand-rolled `build_virtual_source`. Retire the fork via upstream PR or their own
oxc 0.156 bump.

## Risks & mitigations

- **rsvelte diagnostic drift vs installed compiler.** Mitigated by the opt-in gate, the version
  probe, and the Phase 3 parity gate; per-diff triage documented in this file.
- **Fork maintenance.** Rebase the bump branch when adopting upstream changes; small by
  construction (3 error classes); retired via upstream PR or their catch-up bump.
- **Codegen waste making the native path slower than the bridge.** Phase 3 measures before any
  default-flip discussion; if slower, the feature stays opt-in with findings recorded.
- **Git-dep supply chain.** Rev-pinned fork under our control; `Cargo.lock` pins the commit;
  release-tarball builds (`cargo build --release --locked`) fetch it like any git dep — the
  release workflow's tarball smoke-test gains the feature in its build matrix in Phase 3.
- **`svelte.config.js` `compilerOptions` silently ignored.** Prevented by routing rule 3 — such
  components always take the bridge.

## Open questions (Phase 1–2 must answer)

1. `CompileResult` ownership: move to a shared types module, or adapter-local conversion into
   `oxvelte_compiler`'s type? (Lean: re-export from `oxvelte_compiler`; avoid a types crate.)
2. `settings.rsvelte` schema: boolean, or object with an explicit `version` assertion / future
   `mode` ("prefer" vs "only")?
3. Engine lifecycle: one `Engine` in a `OnceLock` (it is stateless) — confirm no per-`prepare`
   cost that argues for pooling.
4. Whether `warningFilter`/`onwarn`-only projects can still use rsvelte compile + bridge callbacks
   (the Phase 3 refinement) — depends on how config detection shakes out in Phase 2.
