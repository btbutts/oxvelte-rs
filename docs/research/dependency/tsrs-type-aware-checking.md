# tsrs type-aware checking — research report

Status: design settled 30OCT2026; implementation in progress on branch
`tsrs-ts6plus` — phase 1 (spike) done 31OCT2026: `src/transport/` framing
landed and the wire format is verified end to end by
`tests/tsrs_wire_protocol.rs`.

### Preface:
Type-aware lint rules (today: `svelte/no-unnecessary-condition`) currently run
through JavaScript embedded in the binary: `src/compiler.rs` spawns a Node
sidecar that loads the *project's own* `typescript` package and answers
checker queries. The proposal: add an optional Cargo feature, `tsrs-inference`,
that compiles support for driving [tsrs](https://github.com/oxc-project/tsrs) —
the Rust port of the TypeScript 7 type checker (Cloudflare/VoidZero, incubated
in oxc) — as an *external* sidecar binary (`tsrs --api`), and routes type
inference for TypeScript 6+ projects through it, natively, with no Node
sidecar. tsrs itself is never compiled into, or by, `oxvelte`: it is a
separate install (Homebrew formula, `cargo install`, or the project's own
`node_modules`) resolved at runtime. The JS path stays for TS ≤ 5 projects
and as a user-controllable fallback.

Rationale for the TS6+ threshold: TS6 is the bridge release — a project
compiling cleanly on TS6 (no bridge deprecation warnings) is TS7-safe, and the
type system is continuous between 6 and 7. So tsrs's TS7-conformant checker is
valid for any TS6-clean project, which is where most development happens going
forward.

### Design shape

- `Cargo.toml`: `tsrs-inference` gates only the support code that locates and
  drives an external `tsrs` binary — same optional, default-off pattern as
  `custom-rules = ["dep:boa_engine"]`, but with **no tsrs crates in our
  dependency graph at all**. The formerly proposed `tsrs-inference-build`
  feature is dropped: `oxvelte` never fetches, compiles, or embeds tsrs.
- The JS scripts (`typescript-service.cjs`, `typescript-conditions.cjs`)
  remain embedded in the binary no matter what.
- Runtime toggle: `settings.tsrs` (boolean, **default `false`**) in
  `oxvelte.config.json`. Compiling the feature in changes nothing on its own:
  with the setting absent or `false`, behavior is identical to today (JS
  sidecar only). `true` opts the project into tsrs routing. The `settings`
  key is already free-form JSON (`src/config.rs:36,110`), so this needs no
  config-schema work.
- Routing per component (only when the feature is compiled in *and*
  `settings.tsrs: true`): Svelte 5 (modern) components in a TS6+ project →
  tsrs path; legacy Svelte or TS ≤ 5 (or missing typescript install, or no
  tsrs binary resolvable) → existing sidecar, unchanged. Enabling the setting
  is deliberately a dual mode, not a switch: legacy language versions always
  keep the JavaScript path.

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
6. ~~**Alternative worth pricing first**~~ — **Chosen 30OCT2026, refined
   31OCT2026.** Keep the sidecar process model but spawn tsrs's API server
   (`tsrs --api`) as an external binary instead of the Node bridge: same
   process model, TS7 speed, zero Rust-embedding risk. This removes blockers
   1 and 4 entirely — we never pin, compile, or embed tsrs crates, and our
   binary doesn't grow. Refined 31OCT2026: we drive the server from our own
   Rust MessagePack client rather than tsrs's JS client SDK (see "Client
   strategy" below), so the type-checking path is JavaScript-free.

### Open questions before implementation

- ~~Does `tsrs --api` expose everything `typescript-conditions` needs~~ —
  **Answered 31OCT2026 (tsrs 0.20.1-ts7.1.0-beta, installed via Homebrew).**
  Yes. `tsrs --api` serves MessagePack over stdio (sync) or JSON-RPC
  (`-async`); upstream's method inventory (npm/sdk/METHODS.md) shows 170/177
  protocol methods answering under tsrs with the pinned TypeScript SDK suite
  passing 968/968, including everything `typescript-conditions.cjs` touches:
  `getTypeAtLocation`, `getBaseConstraintOfType`, `isArrayType`,
  `getPropertyOfType`, `getTypeOfSymbolAtLocation`, `getIndexInfoOfType`,
  `getSignaturesOfType`, `getReturnTypeOfSignature`, `isTypeAssignableTo`,
  plus project plumbing (`parseConfigFile`, `createSnapshot`,
  `getDefaultProjectForFile`). The bare 21 MB binary embeds the
  `lib.*.d.ts` typings — verified: it type-checks a strict project with no
  npm install present.
- ~~What exactly crosses the wire with each answer?~~ — **Answered 31OCT2026
  by spike** (now `tests/tsrs_wire_protocol.rs`, run against the Homebrew
  tsrs 0.20.1-ts7.1.0-beta through our real `src/transport/` framing):
  - `getTypeAtPosition { snapshot, project, file, position }` returns a full
    `TypeResponse` per query: `id` (u32), **`flags` inline** (u32 —
    `TypeFlags`), plus `objectFlags`, `isTupleType`, `intrinsicName`
    (`"number"`, `"string"`, `"null"`, …) and literal `value` (`"oops"` for a
    string literal, with `freshType`/`regularType` handles). Object types
    carry `target` and a `symbol` handle.
  - **Union/intersection parts are not inline** — fetch them with
    `getTypesOfType { snapshot, project, objectId }` (note the field name is
    `objectId`, not `type`), which returns full `TypeResponse`s per part
    (null = flags 0x8, string = flags 0x20 observed).
  - Handles on the wire: `snapshot` u64, `project` string (the tsconfig
    path), `NodeHandle` string, positions are UTF-16 offsets.
  - **TS7's `TypeFlags` values differ from TS ≤ 5** (observed: null 0x8,
    string 0x20, number 0x40, string literal 0x400, union 0x8000000) — the
    conditions port must use the TS7 enum, not values copied from the
    TS5-era `typescript` package.
  - `getSemanticDiagnostics { snapshot, project, files }` returns
    `{ pos, end, startPosition: {line, character}, code, category, text }`.
  - Requests are answered one tuple per request, strictly sequential, no
    server-initiated traffic when `-callbacks` is not enabled.
- ~~Does single-threaded/canonical-history mode guarantee byte-stable
  diagnostics?~~ — **Spot-checked 31OCT2026:** two separate `tsrs --api`
  processes answered the spike's full request sequence byte-identically.
  (Full-project diagnostics determinism still gets its real proof in the
  phase-4 parity run.)
- ~~Policy: TS6+ or TS7-only?~~ — **Settled 31OCT2026: TS6+.** TS6 is the
  bridge release, type-system-continuous with 7. When `settings.tsrs` is
  enabled, TS6+ projects route to tsrs; TS ≤ 5 projects always keep the JS
  sidecar (tsrs cannot serve them — it rejects pre-TS7 options), so the
  embedded JS path is permanent legacy support, not a stopgap.

(Earlier questions about a Rust embedding entry point, minimum crate set, and
locating `lib.*.d.ts` outside npm are moot under the revised design — we never
embed, and the tsrs binary bundles its own libs.)

### Client strategy (settled 31OCT2026)

**Rust-native MessagePack client; the rule logic ports to Rust.** Two
options were priced:

- *JS sidecar + vendored tsrs client SDK* — keep `typescript-conditions.cjs`
  as JS and swap its `typescript` module for tsrs's client SDK (the
  `tsrs/sync` export of the npm package, ~4 MB unpacked, which spawns
  `tsrs --api` itself and accepts `tsserverPath`). Smallest diff, but keeps
  JavaScript in the type-checking path forever.
- *Rust-native* — oxvelte spawns `tsrs --api` and speaks the wire protocol
  directly; `typescript-conditions.cjs` (179 lines) and the type-checking
  half of `typescript-service.cjs` (145 lines: virtual-file construction,
  tsconfig discovery) get Rust counterparts.

**Chosen: Rust-native.** All of our rules are already Rust, tsrs is Rust,
and MessagePack is the protocol we intend to reuse when this binary is
wrapped by a VS Code extension — the client is a foundation, not throwaway.
The embedded JS sidecar stays shipped regardless as the permanent TS ≤ 5
path.

### Wire protocol (verified 31OCT2026 against upstream sources)

Read from `crates/tsrs_api_transport/src/msgpack.rs` and its upstream
original, `microsoft/TypeScript tsc/internal/api/protocol_msgpack.go`:

**The "MessagePack protocol" is not generic MessagePack data — no MessagePack
crate applies.** The wire format is a hand-rolled tuple envelope:

```
[MessageType, method (bin), payload (bin)]
```

- A fixed 3-element array marker (`0x93`), the message type as a positive
  fixint (or `uint8`), then two length-prefixed binary fields (`bin8/16/32`,
  markers `0xC4`–`0xC6`). Those five byte constants are the *entirety* of
  MessagePack on the wire.
- **Payloads are JSON text**, not MessagePack: upstream marshals params and
  results with `json.Marshal` into the payload bin (with a `RawBinary`
  escape hatch). So the struct-as-array vs struct-as-map differences between
  `rmp-serde`, `messagepack-serde`, and `msgpacker` never apply — structs
  never pass through MessagePack at all.
- There are **no request IDs**: requests and responses are matched by method
  name, so requests on one connection are strictly sequential.
- Message types (client view): we send `Request` (1); the server answers
  `CallResponse` (2) or `CallError` (3, payload is a plain error string). The
  server may send `Call` (6) tuples for FS callbacks when `-callbacks` is
  enabled — we answer those with `Response` (4) / `Error` (5).

**Decision: hand-port the framing, use `serde_json` for payloads.** The
framing is ~150 lines of pure `std::io` code with no dependencies; porting
tsrs's `msgpack.rs` (Apache-2.0 — credit in `THIRD_PARTY_NOTICES`) gives us
byte-exact wire compatibility by construction, while every payload goes
through the `serde`/`serde_json` stack we already have. Adding `rmp-serde`
or any MessagePack crate would buy us five byte constants and pull in
serializers we would never call.

### Source layout (settled 31OCT2026)

All gated behind `#[cfg(feature = "tsrs-inference")]`:

```
src/
  transport/
    mod.rs            — framing-agnostic shared pieces: TransportError,
                        Message/Response, DEFAULT_MAX_FRAME_BYTES,
                        read_exact_bounded. The future VS Code extension
                        transport reuses this module.
    message_pack.rs   — the tuple framing above (port of tsrs's msgpack.rs):
                        MessageType, Tuple, MessagePackReader/Writer
    json_rpc.rs       — later: the `-async` JSON-RPC framing, added when the
                        VS Code extension work begins (different message
                        shapes, same transport/mod.rs base)
  tsrs/
    mod.rs            — TsrsClient: spawn `tsrs --api`, session lifecycle
                        (initialize, createSnapshot/updateSnapshot, close),
                        sequential request/response, FS-callback replies
    resolve.rs        — binary resolution (project node_modules/.bin walked
                        up from the linted file, then PATH; never cached) and
                        project TS-version detection (nearest
                        node_modules/typescript/package.json)
    protocol.rs       — serde types for the methods we call: params and
                        result shapes (handles, flags, spans)
    conditions.rs     — the no-unnecessary-condition checks ported from
                        typescript-conditions.cjs, driven through TsrsClient;
                        no_unnecessary_condition.rs dispatches here for
                        TS6+ components when settings.tsrs is on
```

The VS Code extension channel lands later as its own module on top of
`src/transport/` — same framing base, different datasets, exactly the
separation the layout above is built for.

Implementation phases:

1. ~~**Spike**~~ — done 31OCT2026 (see "Open questions" for findings):
   `src/transport/{mod.rs,message_pack.rs}` landed behind the
   `tsrs-inference` feature, and the spike became the feature-gated smoke
   test `tests/tsrs_wire_protocol.rs` (skips when no `tsrs` is on PATH);
   framing, flags, union-part fetching, and diagnostics verified against the
   installed binary.
2. **Plumbing** — `TsrsClient` spawn/lifecycle, binary
   resolution (project `node_modules/.bin` walked up, then PATH, never
   cached), project TS-version detection (nearest
   `node_modules/typescript/package.json`), the `settings.tsrs` gate, the
   `tsrs-inference` feature flag.
3. **Ports** — virtual-file construction + tsconfig setup (from
   `typescript-service.cjs`) and the conditions logic (from
   `typescript-conditions.cjs`) as a tsrs backend for
   `no_unnecessary_condition.rs`. The Svelte-specific exclusions (mutable
   root bindings, `$:` blocks) already live in our Rust rule and don't move.
4. **Parity proof** — byte-exact diagnostics vs the JS sidecar across the
   version-matrix before the feature is considered reliable.

### Test strategy (settled 31OCT2026)

- **Features are always on for dev builds and tests; release stays
  opt-in.** `Cargo.toml` carries `[dev-dependencies] oxvelte = { path = ".",
  features = ["tsrs-inference", "custom-rules"] }` — a dev-only
  self-dependency that turns both features on for every `cargo test` and
  dev build with **no `--features` flags needed anywhere in the dev
  workflow** (and, as a side effect, makes rust-analyzer see the
  feature-gated code). The `release` profile is unaffected: features remain
  explicit opt-ins there (verified: successive `cargo build --release`
  runs with/without the flags compile different step counts; `cargo test
  --lib` grew 648 → 649 the moment the trick landed, as the previously
  cfg-gated custom-rules test activated). Compiled-in never means enabled:
  `settings.tsrs` (default `false`) remains the runtime switch.
- **tsrs-specific tests** are gated with `#![cfg(feature = "tsrs-inference")]`
  (whole-file, like `tests/tsrs_wire_protocol.rs`) and **skip gracefully
  when no `tsrs` binary is on PATH** — no developer or CI worker is forced
  to have tsrs installed.
- **Dual-backend fixture runs are separate test entries, generated, not
  hand-duplicated.** The fixture harness has one `#[test]` per rule+kind
  (`linter_<rule>_valid` / `linter_<rule>_invalid`) iterating that rule's
  manifest cases through shared helpers. For each type-aware rule eligible
  for both paths (Svelte 5 + TS6+), we add a parallel feature-gated
  `#[test]` (e.g. `linter_no_unnecessary_condition_invalid_tsrs`) that runs
  the *same* case-iteration helper with `settings.tsrs` forced on — the
  case loop and expectations are written once; only a two-line test entry
  is added per rule+kind. Each backend's run is its own line in the test
  report, so the count grows visibly (today: one type-aware rule → +2
  entries, valid and invalid; N eligible rules → +2N). Expectations must
  match byte-for-byte on both backends; a divergence is a parity failure,
  not a per-backend expectation file. Non-type-aware rules never touch a
  type backend and keep exactly one entry each.

### Recommendation

~~Park implementation until tsrs publishes crates~~ ~~Superseded 25OCT2026:
build-time sidecar binary~~ **Settled 30OCT2026:** proceed with the external
sidecar binary design below. The `tsrs-inference` feature flag (support code
only), the `settings.tsrs` toggle (default off), the TS-version routing, and
the JS-sidecar fallback are all compatible with today's architecture, and
installing tsrs is always the user's or the package manager's concern — never
our build's. The version-matrix gives us the harness to prove byte-exact
diagnostics under both paths before we ever consider widening the default.

### ~~Interim approach: build-time sidecar binary~~ (dropped 30OCT2026)

The 25OCT2026 proposal to have `build.rs` fetch and compile tsrs under a
`tsrs-inference-build` feature is dropped. oxvelte never compiles tsrs, in any
configuration: tsrs is a standalone tool with its own distribution channels
(Homebrew formula, `cargo install`, npm prebuilt binaries), and treating it
as one is simpler, keeps our build times and binary size flat, and sidesteps
the cross-compilation and `OUT_DIR`-caching problems that proposal carried.
The verified size facts (~21 MB prebuilt binary plus ~3–4 MB of bundled
`lib.*.d.ts` typings) still stand — they are simply shipped by tsrs's own
packages now, not ours.

### Distribution design (revised 30OCT2026)

One Cargo feature, and tsrs is never compiled as part of oxvelte:

| Feature | Effect |
|---|---|
| `tsrs-inference` | Compiles the Rust support code that locates and drives an external `tsrs` binary (`tsrs --api`). Never fetches, compiles, or embeds tsrs itself. |

`settings.tsrs` defaults to `false`, so a binary built with the feature
behaves exactly as today's until the user opts in — compiling in tsrs support
does **not** enable it for TS6+ projects by itself.

Per channel:

- **Homebrew tap** — `tsrs` ships as its own formula in our tap, and the
  `oxvelte` formula declares it as a dependency. The oxvelte formula builds
  with `--features tsrs-inference` only. Homebrew links both binaries into
  `bin/` on PATH, which the runtime resolution below finds.
- **`cargo build` / `cargo install` from source** — built with
  `--features tsrs-inference`; the user installs tsrs separately (the
  Homebrew formula, `cargo install`, or npm into their project). tsrs is a
  full TS7 compiler in its own right, so having it as a standalone binary is
  a feature, not a burden.
- **npm / bun** — the prebuilt oxvelte binaries in `release.yml` are compiled
  with `--features tsrs-inference` (support code only, no tsrs build step).
  Our package does **not** declare `tsrs` as a dependency: users who want it
  install it into their project from npm themselves (where its own
  optionalDependencies fetch the correct prebuilt `@tsrs/<platform>` binary),
  or rely on a PATH install. Bun installs from the same npm registry, so bun
  users get identical behavior.

**Runtime resolution order** in the oxvelte binary (when the feature is
compiled in *and* `settings.tsrs: true`):

1. The `tsrs` binary in the project's `node_modules/.bin/` (walked up from
   the linted file, the same anchoring the bridge uses for `svelte/compiler`
   and `typescript` today). A project-level install always wins, and its
   presence is recorded in the project's own `package.json`.
2. `PATH` — covers the Homebrew formula, `cargo install`, and anyone who
   installed tsrs themselves. (The Homebrew layout needs no special case:
   both binaries land in the same `bin/`, which is already on PATH.)

Resolution runs on every invocation and is never cached: if a user who was
running a PATH-installed tsrs later installs tsrs into their project's
`node_modules/`, the very next `oxvelte` run prefers the project copy
automatically. If `settings.tsrs` is `true` but no binary resolves at either
location, warn once and fall back to the JS sidecar. An explicit path setting
in `oxvelte.config.json` was considered and judged unnecessary given the
above; it can be added later if a real need appears.
