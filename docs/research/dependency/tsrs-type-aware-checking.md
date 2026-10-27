# tsrs type-aware checking — research report

Status: research complete 25OCT2026, awaiting decision. No code changes yet.

### Preface:
Type-aware lint rules (today: `svelte/no-unnecessary-condition`) currently run
through JavaScript embedded in the binary: `src/compiler.rs` spawns a Node
sidecar that loads the *project's own* `typescript` package and answers
checker queries. The proposal: add an optional Cargo feature, `tsrs-inference`,
that compiles [tsrs](https://github.com/oxc-project/tsrs) — the Rust port of
the TypeScript 7 type checker (Cloudflare/VoidZero, incubated in oxc) — into
`oxvelte` and routes type inference for TypeScript 6+ projects through it,
natively, with no sidecar. The JS path stays for TS ≤ 5 projects and as a
user-controllable fallback.

Rationale for the TS6+ threshold: TS6 is the bridge release — a project
compiling cleanly on TS6 (no bridge deprecation warnings) is TS7-safe, and the
type system is continuous between 6 and 7. So tsrs's TS7-conformant checker is
valid for any TS6-clean project, which is where most development happens going
forward.

### Design shape (as proposed)

- `Cargo.toml`: `tsrs-inference = ["dep:tsrs_compiler", ...]` — same optional,
  default-off pattern as `custom-rules = ["dep:boa_engine"]`. Crates would be
  git-pinned (see risks: nothing is on crates.io).
- The JS scripts (`typescript-service.cjs`, `typescript-conditions.cjs`)
  remain embedded in the binary no matter what.
- Runtime toggle: `settings.tsrs` (boolean) in `oxvelte.config.json`. When the
  feature is compiled in: default on for eligible projects; `false` forces the
  JS sidecar — an escape hatch if tsrs misbehaves on a given project. The
  `settings` key is already free-form JSON (`src/config.rs:36,110`), so this
  needs no config-schema work.
- Routing per component: TS6+ project → tsrs native path; TS ≤ 5 (or missing
  typescript install, or opt-out) → existing sidecar, unchanged.

### What the current path does (what a native path must replicate)

Read from `no_unnecessary_condition.rs`, `src/compiler.rs`,
`typescript-service.cjs`, `typescript-conditions.cjs`:

1. The rule sends the whole `.svelte` source plus UTF-16 spans for each
   `<script>` block and each template expression (`{#if}` tests as
   `condition`, mustache/attribute/each expressions as `expression`).
2. The sidecar builds a **virtual `.ts` file**: every non-script character is
   blanked to spaces (offsets stay 1:1 with the source), scripts restored
   verbatim, template expressions appended as `if (<expr>) {}` / `void
   (<expr>);` with a projection table mapping appended offsets back to the
   original source.
3. It discovers tsconfig (`settings.typescript.project` override, else
   `ts.findConfigFile` upward from the file), forces `noEmit/allowJs/checkJs`,
   and creates a `ts.Program` over the project files plus the virtual file.
4. `typescript-conditions.cjs` is a faithful port of typescript-eslint's
   `no-unnecessary-condition`: `TypeFlags` bit tests over union parts —
   always-truthy/falsy conditions, never-nullish `??` operands, unnecessary
   `?.` (with a fix), literal/no-overlap comparisons, always-truthy/falsy
   callback returns on array methods, a whole-file strictNullChecks gate.
   Svelte-specific exclusion: nodes inside top-level `$:` blocks or template
   regions that reference **mutable root bindings** are skipped, because
   Svelte reactivity makes those types flow-sensitive in ways the checker
   can't see.
5. Offsets cross the wire as UTF-16; the rule converts back to byte spans.
   Sessions are built per request — no cross-request program caching today.

Every one of these steps has a direct counterpart in tsrs's port (it is a
function-by-function port, so `getTypeAtLocation`, `getBaseConstraintOfType`,
`isArrayType`, call-signature queries, etc. all exist).

### tsrs facts (verified 25OCT2026)

Sources: [repo README](https://github.com/oxc-project/tsrs),
[crates/ listing](https://github.com/oxc-project/tsrs/tree/main/crates),
[COMPATIBILITY.md](https://github.com/oxc-project/tsrs/blob/main/docs/COMPATIBILITY.md),
[NODE_API.md](https://github.com/oxc-project/tsrs/blob/main/docs/NODE_API.md),
[RUST.md](https://github.com/oxc-project/tsrs/blob/main/docs/RUST.md),
[STATUS.md](https://github.com/oxc-project/tsrs/blob/main/docs/STATUS.md),
[bench/RESULTS.md](https://github.com/oxc-project/tsrs/blob/main/bench/RESULTS.md).

- **Crates exist as the user described**: ~37 workspace crates including
  `tsrs_api`, `tsrs_core`, `tsrs_compiler`, `tsrs_checker`, `tsrs_parser`,
  `tsrs_binder`, `tsrs_tsoptions`, `tsrs_vfs`, `tsrs_lsp`, `tsrs_cli`, …
- **But nothing is published.** `docs/RUST.md` states all workspace crates
  declare `publish = false`; crates.io has nothing from the project.
  Consumption today means a **git dependency pinned to a commit**, and
  `tsrs_api` is the *server-side implementation* of their Node API, not a
  consumer SDK. There is no documented Rust embedding surface; internals
  mirror tsgo (arena allocation, 32-bit handles, `'static` returns,
  ~500 audited `unsafe` sites).
- **Maturity**: last release 0.18.0; 0.19.0 targeting a TS 7.1.0-beta pin was
  in prep as of 25OCT2026. Conformance at the pin: all 13,700 error baselines
  and 12,998 `.types`/`.symbols` baselines match tsgo; 4,092/4,562 Fourslash
  groups. Gaps: no `--init`/`--showConfig`/tracing, no watch on Windows, and
  by default it *preserves* TypeScript's checker-count/file-order diagnostic
  sensitivity (the opt-in `--fast` preset changes scheduling further). No
  stability commitment; PRs closed, issues open.
- **Performance vs tsgo 7.0.2**: ~4–17x wall time, ~2–4.6x less peak RSS at
  matched threads (vendor numbers; independent tests land closer to 3–5x).
- **License**: Apache-2.0 (derivative of TypeScript) — compatible with MIT.
- **In-memory checking works**: the API/LSP layer supports unsaved/virtual
  files (`createSnapshot`), so our blanked-virtual-file trick maps on
  directly. We don't need their `contentMappers` (`.svelte` plugin point) —
  we synthesize the virtual `.ts` ourselves.
- **TS 7.x semantics only**: tsrs pins tsgo 7.1.0-beta and, like tsgo,
  *rejects* options TS7 removed (ES5 target, AMD/UMD/System, `baseUrl`, …).
  It cannot serve TS ≤ 6 projects with their own checker's semantics.
- Project TS-version detection at lint time is easy: walk up from the
  component to the nearest `node_modules/typescript/package.json` and parse
  `version` — the same anchor the sidecar's `createRequire` already uses.

### Verdict

**Technically feasible; packaging is the blocker, not capability.** The
analysis we need is fully portable, virtual files are supported, the license
is fine, and the feature-flag + settings-toggle design slots into existing
patterns cleanly. What makes this "research, not implementation" today:

1. **No published crates, no supported Rust API** — a git pin to a
   fast-moving, PRs-closed, 37-crate workspace. Any upstream refactor breaks
   us and can't be upstreamed. Watch the 0.19.x line for an embedding story.
2. **Version skew by design** — tsrs checks with TS 7.1-beta semantics while a
   project may pin TS 6.x/7.0.x; upstream's own benches show real error-count
   drift between 7.0.2 and 7.1-dev. Since our rule's diagnostics are
   type-derived, results can differ from the project's own tsc. The routing
   threshold (TS6+ vs TS7-only) needs an explicit policy decision informed by
   this.
3. **Determinism** — we'd force single-checker/canonical-history mode; default
   multithreaded checking inherits TS's order-sensitive diagnostics, which is
   unacceptable for lint output.
4. **Binary size / build time** — embedding a full TS7 compiler plus the
   `lib.*.d.ts` typings dwarfs the current binary (cf. boa added ~2 MB; this
   is a much larger embed). `build.rs` already vendors assets, so the libs
   could be embedded the same way. Upstream's numbers also rely on PGO/BOLT;
   plain `cargo build --release` runs ~7–14% slower than their published
   builds.
5. **Ergonomics** — arena-based `'static` internals suit a
   "create arena → build program → query → drop" per-request pattern, which
   matches current behavior but forfeits tsrs's headline watch-mode gains. A
   persistent incremental session is a larger design.
6. **Alternative worth pricing first** — keep the sidecar but let it spawn
   tsrs's **Node API** (`tsrs --api`) when the project has tsrs installed:
   same process model, TS7 speed, zero Rust-embedding risk. Doesn't remove
   Node, so it only helps if the goal is speed rather than dependency
   elimination.

### Open questions before implementation

- Does (or will) tsrs expose a documented "check these virtual files with this
  tsconfig" Rust entry point?
- What is the minimum crate set? (`tsrs_checker` pulls most of the workspace
  transitively.)
- How are the bundled `lib.*.d.ts` typings located outside an npm install?
- Does single-threaded mode guarantee byte-stable diagnostics across identical
  runs?
- Policy: is the routing threshold TS6+ (user's proposal) or TS7-only, given
  the 7.0→7.1 semantic drift tsrs tracks?

### Recommendation

~~Park implementation until tsrs publishes crates~~ **Superseded 25OCT2026:**
we proceed with the build-time sidecar binary below rather than wait for
published crates, and swap to plain `dep:` dependencies under the same feature
flag if/when a documented Rust embedding API ships. The `tsrs-inference`
feature flag, the `settings.tsrs` toggle, the TS-version routing, and the
sidecar fallback are all compatible with today's architecture. The
version-matrix gives us the harness to prove byte-exact diagnostics under
both paths before we ever ship it default-on.

### Interim approach: build-time sidecar binary (proposed 25OCT2026)

Rather than wait for published crates, the first implementation compiles the
tsrs **CLI binary at build time** and has `oxvelte` spawn `tsrs --api` as a
sidecar (same process model as the Node bridge today). Sketch: `build.rs`
checks `CARGO_FEATURE_TSRS_INFERENCE`; when set, it fetches tsrs at a pinned
tag (reuse the tarball machinery we already built for the CSS sources — the
pin lives in `.cargo/config.toml` like the other pins), builds it with cargo,
and hands the binary path to the main crate via `cargo:rustc-env`. When crates
are eventually published, this swaps to plain `dep:` dependencies under the
same feature flag with no user-facing change.

**Size facts (verified 25OCT2026 via the npm registry):** the prebuilt tsrs
platform binary unpacks to **~21 MB** (`@tsrs/darwin-arm64`), plus the bundled
`lib.*.d.ts` typings (~3–4 MB) that any real type-check needs. So:

- *Sidecar-binary approach*: our binary doesn't grow at all, but we ship/build
  a second ~21 MB executable.
- *Crate-embedding approach (later)*: we could skip the CLI/LSP/WASM crates,
  but the checker pulls most of the 37-crate workspace transitively, and the
  lib typings are always required — expect most of that ~21 MB regardless.
  You cannot subset a prebuilt binary, and you cannot type-check without the
  libs.

**Refinements needed on the sketch before it's buildable:**

1. **Don't cache the built binary only in `OUT_DIR`.** `OUT_DIR` dies with
   `cargo clean`, which would force a full tsrs rebuild (a 37-crate release
   build — many minutes) after every clean. Cache the built binary in a
   stable location keyed by the pin (same philosophy as
   `resources/css/known_css_properties.json`), and only copy/link it into
   `OUT_DIR` per build.
2. **`cargo install --git` ignores our lockfile and resolves fresh**; driving
   a plain `cargo build --release` inside an extracted tarball at the pinned
   tag is more reproducible and reuses our existing `fetch_github_repo`
   helper. Nested cargo from a build script is workable (the outer build
   doesn't hold the package-cache lock while scripts run) but blocks other
   cargo builds and makes first builds slow — document that expectation.
3. **Cross-compilation breaks the naive version.** `release.yml` builds 8
   targets; `cargo install` builds for the *host*. `build.rs` receives
   `TARGET`, so the tsrs build would have to be invoked with `--target` (and
   the toolchains present) — doable, but the real pain is below.
4. ~~**Distribution:** a `cargo:rustc-env=TSRS_BIN_PATH=...` path only exists on
   the machine that compiled it.~~ Resolved by the distribution design below.

### Distribution design (settled 25OCT2026)

Whether `build.rs` compiles tsrs at all depends on how the user obtained
oxvelte. Two Cargo features drive it (features are additive, so
`tsrs-inference-build` implies `tsrs-inference`):

| Feature | Effect |
|---|---|
| `tsrs-inference` | Compiles the Rust support code that knows how to locate and drive a `tsrs` binary (`tsrs --api`). Never compiles tsrs itself. |
| `tsrs-inference-build` | Implies `tsrs-inference`; `build.rs` additionally fetches the pinned tsrs tarball (pin in `.cargo/config.toml`) and runs `cargo build --release` on it, producing the standalone `tsrs` binary. |

Per channel:

- **`cargo build` / `cargo install` from source** — user opts in with
  `--features tsrs-inference-build`; `build.rs` compiles tsrs from the pinned
  tarball (cached in a stable location keyed by the pin, per refinement #1).
  Getting the resulting binary onto PATH is an installer concern, not a
  build-script concern: `cargo install` only installs a package's own
  declared bins, so source-build users either run `cargo install tsrs
  --git …` themselves or add the built binary to PATH. tsrs is a full TS7
  compiler in its own right, so having it as a standalone binary is a
  feature, not a burden.
- **Homebrew tap** — builds from source like the above; the formula passes
  `--features tsrs-inference-build` and installs *both* binaries into
  `bin/`, which also satisfies the same-directory resolution rule below.
- **npm / bun** — tsrs is **never compiled** when building for npm
  distribution. Instead our package declares `tsrs` as a normal dependency
  in `package.json` (staged by `scripts/stage-npm.mjs`); npm/bun then fetch
  the correct prebuilt `@tsrs/<platform>` binary via tsrs's own
  optionalDependencies — their distribution machinery, not ours. The
  prebuilt oxvelte binaries in `release.yml` are compiled with
  `--features tsrs-inference` (support code only, no tsrs build step).
  Bun installs from the same npm registry, so bun users get identical
  behavior.

**Runtime resolution order** in the oxvelte binary (when the feature is
compiled in and `settings.tsrs` is not `false`):

1. The `tsrs` binary in the project's `node_modules/.bin/` (walked up from
   the linted file, the same anchoring the bridge uses for `svelte/compiler`
   and `typescript` today) — this is the npm/bun path.
2. The directory containing the running `oxvelte` executable — covers the
   Homebrew layout where both binaries land in `bin/` together.
3. `PATH` — covers source builds and anyone who installed tsrs themselves.

An explicit path setting in `oxvelte.config.json` was considered and judged
unnecessary given the above; it can be added later if a real need appears.
`settings.tsrs: false` forces the JS sidecar regardless of how tsrs was
installed.
