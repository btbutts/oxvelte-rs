# Session handoff — tsrs implementation (31OCT2026)

Written for a fresh session to pick up the `tsrs-ts6plus` branch work without
re-reading the full chat history. Read this first, then
`docs/research/dependency/tsrs-type-aware-checking.md` (the living planning
doc, kept current through phase 3).

## CRITICAL: unverified state at handoff

The session was interrupted mid-task. The last command
(`rm -rf fixtures/tsrs && cargo test --test tsrs_conditions`) **never ran**
(the approval was cancelled). Therefore:

1. `tests/tsrs_conditions.rs` was **completely rewritten** (upstream-corpus
   version) but has **never been compiled or run**. It may have compile
   errors or logic bugs. First action in a new session:
   `cargo test --test tsrs_conditions` (see shell notes below) and fix
   whatever surfaces.
2. `fixtures/tsrs/` **may still exist** — delete it if so (it was fully
   superseded by the fixtures/linter approach; the rewritten test no longer
   references it).
3. Everything before that point (all of phases 1–3 and their commits) was
   verified green. Only the final dual-backend test rework is unproven.

## Environment essentials

- Working repo: `/Users/brianbutts/Documents/GitHub Projects/oxvelte-rs`,
  branch `tsrs-ts6plus`. (The user's shell CWD may be a *different* repo,
  `oxvelte-tap` — always use absolute paths or `cd` into oxvelte-rs.)
- `cargo` is NOT on PATH in fresh shells: prefix with
  `PATH="$HOME/.cargo/bin:$PATH"`.
- Machine has bun 1.4.4 and node 26; `tsrs` 0.20.1-ts7.1.0-beta installed at
  `/opt/homebrew/bin/tsrs` via the user's own Homebrew tap (binary embeds
  lib.*.d.ts typings).
- Dev/test builds have `tsrs-inference` + `custom-rules` features ON via a
  dev-only self-dependency in `Cargo.toml` `[dev-dependencies]`
  (`oxvelte = { path = ".", features = [...] }`) — no `--features` flags
  needed for `cargo test`; **release builds keep features opt-in**. NB:
  plain `cargo check --lib` does NOT enable dev-deps features — use
  `cargo check --lib --features tsrs-inference` or just run tests.
- User commits themselves; provide commit messages in their style (wrapped
  subject + per-file bullet sections + verification paragraph, exemplified
  by commit `e5dcebc`).
- Conventions: `cargo fmt --check` and `cargo clippy --all-targets` must be
  clean; all suites green before claiming done; tests that need the tsrs
  binary skip silently when it's absent; user dislikes `shopt` and
  unfamiliar idioms — keep bash and Rust simple and explicit.

## Where the implementation stands

**Phases 1–3 complete and committed.**

- `src/transport/{mod.rs,message_pack.rs}` — the `[MessageType, method,
  payload]` tuple framing ported from tsrs's `msgpack.rs` (Apache-2.0, in
  THIRD_PARTY_NOTICES). Payloads are JSON inside the framing; raw-binary
  responses handled too. No MessagePack crate — payloads use serde_json.
- `src/tsrs/ts_locator.rs` — **renamed from `resolve.rs` this session**
  (user request): `find_tsrs_binary` (project `node_modules/.bin` walked
  up, then PATH, never cached), `project_ts_major_version` (nearest
  `node_modules/typescript/package.json`), `is_tsrs_eligible` (major >= 6).
- `src/tsrs/protocol.rs` — serde types (TypeResponse with inline flags,
  SymbolResponse+SymbolReference, SignatureResponse{id}, DiagnosticResponse).
- `src/tsrs/mod.rs` — `TsrsClient` (spawn `tsrs --api -cwd`, initialize
  handshake storing case_sensitivity, `request`/`request_raw`, typed helpers
  incl. get_type_at_location, get_types_of_type, get_base_constraint_of_type,
  is_array_type, get_property_of_type, get_type_of_symbol_at_location,
  get_index_info_of_type, get_signatures_of_type,
  get_return_type_of_signature, get_syntactic/semantic_diagnostics,
  get_shorthand_assignment_value_symbol; kill-on-drop).
- `src/tsrs/session.rs` — `TsrsSession`: virtual-.ts construction (blanking
  + projections, UTF-16), tsconfig discovery, two-step snapshot
  (openProjects → parsed options, then createPrograms with a
  `kind:"layer"` filesystem injecting the virtual file). Wire findings:
  snapshots are immutable (updateSnapshot returns the NEW snapshot id);
  parsed tsconfig options accepted verbatim as createPrograms options.
- `src/tsrs/binary_ast.rs` — reader for getSourceFile's binary AST:
  64-byte header, nodes-section offset at bytes 40..44, fixed 28-byte node
  records (kind/pos/end/...). Node handle = `index.kind.pathKey`. Record pos
  includes trivia — `skip_trivia` reproduces getStart in UTF-16 units.
  getSourceFile returns RAW BINARY on the sync protocol.
- `src/tsrs/conditions.rs` — full port of typescript-conditions.cjs: oxc
  walks the virtual TS AST (`oxc::ast_visit::Visit`, infallible — errors
  accumulate in self.error); tsrs answers type/symbol queries. Findings:
  chain-element members/calls route through `visit_member_expression` /
  `visit_call_expression` (NOT visit_expression); boolean literal types
  carry `value: true/false` with NO intrinsicName on the wire (unlike TS5).
  TS7 TypeFlags renumbered from TS5 (null=0x8, string=0x20, number=0x40,
  string literal=0x400, union=0x8000000 — from tsc/internal/checker/types.go).
- `src/linter/rules/no_unnecessary_condition.rs` — routes to tsrs only when:
  feature compiled in + `settings.tsrs: true` + Svelte 5
  (ctx.svelte_version.includes_major(5)) + TS6+ + binary resolvable. All
  else = unchanged JS sidecar.
- **Param names are inconsistent upstream**: getTypesOfType and
  getReturnTypeOfSignature take `objectId`; getBaseConstraintOfType,
  isArrayType, getPropertyOfType(+name), getIndexInfoOfType(+kind),
  getSignaturesOfType(+kind) take `type`. getSignaturesOfType returns
  objects {id}, not bare u64s.

## This session's most recent (partially verified) changes

- **import.mjs** (scripts/get-fixtures/): env overrides for the parity
  environment versions (`OXVELTE_FIXTURES_SVELTE_VERSION`,
  `OXVELTE_FIXTURES_TYPESCRIPT_VERSION`, added to `.cargo/config.toml` as
  5.49.2 / 6.0.3); patches `fixtures/linter/upstream/tsconfig.json`
  `target: es5 → es6` after download (before hashing, so the manifest hash
  stays consistent — user decided this is acceptable; es2015/ES6 is the
  real floor, and tsrs rejects the ES5 *emit target*); writes
  `fixtures/linter/package.json` with both svelte+typescript deps; writes
  version-marker `fixtures/linter/node_modules/typescript/package.json`
  (gitignored via new `/fixtures/linter/node_modules/` entry). Ran
  `cargo get-eslint-fixtures` — regenerated fine, tsconfig now es6.
- `tests/tsrs_conditions.rs` — **REWRITTEN, UNVERIFIED** (see critical
  note). Two-tier coverage: 11 inline cases (every message kind + reactive
  exclusion + negatives) against the fixtures/linter project, PLUS the 7
  upstream manifest cases for the rule. Five test fns:
  inline/upstream × js/tsrs + backends_agree_on_diagnostics_and_spans.
  Earlier inline-only version of this file PASSED all 3 tests (that version
  used the now-deleted fixtures/tsrs project; the JS sidecar needs
  OXVELTE_COMPILER_RUNTIME — the test sets it to scripts/compiler-runtime
  via std::sync::Once).

## Remaining work

1. **Verify `cargo test --test tsrs_conditions`**; delete `fixtures/tsrs/`
   if still present. Then commit (message in user style).
2. **Phase 4** (from the planning doc): byte-exact parity proof across the
   version-matrix (scripts/version-matrix/) before the feature is trusted.
3. **README documentation** for `settings.tsrs` (default false, opt-in,
   Svelte 5 + TS6+ only, resolution order node_modules→PATH, JS sidecar is
   the permanent TS ≤ 5 / disabled path).
4. Future (deferred, documented): watch-mode service CLI; VS Code extension
   transport on `src/transport/json_rpc.rs` (the layout was designed for
   this: transport base shared, tsrs client and extension as siblings).
5. Possible follow-up: `release.yml` builds 8 targets — if/when the
   prebuilt binaries should ship with tsrs support compiled in, add
   `--features tsrs-inference` there (feature is default-off).

## Test counts at handoff

661 lib tests (12 tsrs unit/integration tests among them), parser_lint_contract
9, upstream_adapter 13, jobsys_regressions 16, upstream parity 1316 executed
with no new gaps, tsrs_wire_protocol 1. All green as of the last full sweep —
EXCEPT the unverified rework noted above.
