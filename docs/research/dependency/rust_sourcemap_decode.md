# Plan: move source-map decode & position remap from JS to Rust

**Status:** planned 02NOV2026
**Scope:** `scripts/compiler-runtime/bridge.cjs`, `scripts/compiler-runtime/vendor/sourcemap-codec.cjs`,
`crates/oxvelte_compiler/`, `crates/oxvelte_compiler/tests/`, `scripts/compiler-runtime/runtime.test.mjs`,
`.github/release-files.txt`
**Goal:** delete the only third-party vendored code embedded in the `oxvelte` binary by moving
source-map decoding and warning-position remapping into Rust. The vendored
`vendor/sourcemap-codec.cjs` stays in the repository as a **development-only test oracle**; it is
no longer embedded, shipped in release tarballs, or executed when an end user runs `oxvelte`.

This is independent of Svelte version and TypeScript version: the codec path runs whenever the
bridge transforms a region (any `lang="ts"` script via the project's TypeScript package, Babel
scripts, and `scss`/`less`/`stylus`/`postcss` styles). It is orthogonal to `tsrs-inference`.

## What does NOT change (settled, do not re-litigate)

- **`bridge.cjs` stays.** Its primary job is running the *user's* npm packages — `svelte/compiler`,
  preprocessors, Babel/TypeScript, `svelte.config.js` hooks. Those are irreducibly JavaScript; no
  Rust replacement exists for `svelte/compiler`. This plan shrinks the bridge; it does not remove it.
- **The source maps themselves are still produced in JS** by sass/less/stylus/postcss/typescript/
  babel. We move the *consumer* (decode + remap), not the producers.
- **`typescript-conditions.cjs` / `typescript-service.cjs` stay embedded** in every build:
  `no_unnecessary_condition.rs` falls back to the JS sidecar whenever tsrs routing declines
  (Svelte 4, TS ≤ 5, `settings.tsrs` off, no tsrs binary).
- **No tsrs crates are imported.** `tsrs_sourcemap` (evaluated and rejected) is a port of
  TypeScript-Go's generator/mapper tied to `tsrs_core`/`tsrs_ast` types; importing it would reverse
  the deliberate "tsrs as external sidecar binary" architecture for a ~270-line decoder shaped for
  someone else's types.
- **No new crate.** The decoder's only consumer is `oxvelte_compiler`, so it lives there as a
  module (see "Home"). `oxvelte_transport` was considered and rejected: it is protocol plumbing
  shared with tsrs and the future VS Code channel, neither of which decodes VLQ source maps.
- **No new dependencies.** Base64-VLQ decoding is written from the public spec
  (tc39.es/ecma426). `oxc_sourcemap` is already in the tree via the oxc transformer chain, but its
  lookup semantics do not match our custom remap (below), so we use neither it nor anything else.

## Current state

Wire path today (`crates/oxvelte_compiler/src/lib.rs`, `scripts/compiler-runtime/bridge.cjs`):

1. `Runtime::start` embeds four JS files into one bootstrap: `vendor/sourcemap-codec.cjs`,
   `typescript-service.cjs`, `typescript-conditions.cjs`, `bridge.cjs` (lib.rs:155-173). The
   codec is exposed to the bridge as `embeddedSourceMapCodec` (bridge.cjs:8-9 falls back to
   `require('./vendor/sourcemap-codec.cjs')` in standalone/test mode).
2. `bridge.cjs` transforms style/script regions. Each transform captures its raw `mappings` VLQ
   string and immediately decodes it (`sourceMapRemap(output, input, mappings,
   sourceMapCodec.decode)`, bridge.cjs:209,250).
3. The bridge composes transformed + untouched regions into `code` and builds a region chain
   `maps` (bridge.cjs:252-262), then remaps every Svelte warning position back to original-source
   UTF-16 indices, with two non-standard behaviors that MUST be preserved exactly:
   - **Previous-line fallback:** a generated line with no mappings scans backward to the nearest
     mapped line and uses its last segment, without column interpolation (bridge.cjs:68-73).
   - **Column interpolation:** within a mapped line, the original column is
     `m[3] + (generated_column - m[0])` — a linear extrapolation past the segment, not a standard
     GLB lookup (bridge.cjs:74-75). End positions are adjusted as `remap(end_index - 1) + 1`
     (bridge.cjs:270).
4. `convert_warnings` (lib.rs:266-290) converts the returned UTF-16 indices to UTF-8 byte spans via
   `oxvelte_transport::span::utf16_byte`. The `callbacks` operation then receives warnings whose
   positions are already final — it is unaffected by this plan.

## Design

### Home

New module `crates/oxvelte_compiler/src/sourcemap.rs`, private to the crate (`mod sourcemap;`).
It contains the decoder and the remap ports — one file, one consumer, no public API surface.

### Rust decoder (`decode_mappings`)

Port of the jridgewell `decode` semantics from the spec:

```rust
pub struct Segment {
    pub generated_column: u32,
    pub source_index: u32,
    pub original_line: u32,
    pub original_column: u32,
}
pub fn decode_mappings(mappings: &str) -> Result<Vec<Vec<Segment>>, String>
```

- Per generated line (`;`-separated), segments (`,`-separated) of 1/4/5 base64-VLQ fields with
  running deltas for generated column, source index, original line, original column (and a name
  index we accept and discard, since sass/babel/typescript emit 5-field segments).
- Validation: malformed base64 digits, truncated segments, and field counts other than 1/4/5
  return `Err`, surfacing through the existing `compile(...) -> Result<_, String>` path — never a
  panic, matching how bridge errors propagate today.
- All positions on the wire are UTF-16 code-unit indices, matching the JS side exactly; conversion
  to bytes happens only at the existing `utf16_byte` boundary.

### Rust remap (ports of bridge.cjs logic)

- `line_starts(&str) -> Vec<u32>` — UTF-16 line-start table handling `\r\n` as one break
  (port of bridge.cjs:48-54, operating on `encode_utf16` units).
- `SourceRemap` — per transform: decoded lines + output/input line-start tables; `remap(index)`
  implements the fallback + interpolation semantics verbatim (bridge.cjs:65-77).
- Region chain — rebuilt in Rust from the transform list exactly as bridge.cjs:252-262, including
  the tail region's `end: code.length + 1` allowance. Identity regions offset into `text`; because
  blanking is index-preserving, `text` and `source` share indices, so the strip list is NOT needed
  on the wire.
- Warning remap — port of `warning()` (bridge.cjs:263-272): pass-through when there are no
  transforms; end-position `remap(end - 1) + 1` adjustment; `start ??= end; end ??= start`.

### Protocol change (bridge → Rust)

`bridge.cjs` stops decoding and remapping. The compile response gains raw materials:

```jsonc
{
  "compiler_version": "...", "svelte_major": 5, "kind": "warn",
  "warnings": [ { "message": "...", "code": "...", "start": 123, "end": 130,
                  "filtered": false, "report": null, "metadata": {...} } ],
  "transforms": [ { "start": 40, "end": 96, "output": "...css...", "mappings": "AAAA..." } ],
  "strip_style_elements": [[s, e]]
}
```

- `warnings[].start/end` are now **raw UTF-16 character indices into the composed `code`**, not
  remapped original-source indices. The bridge resolves `character` for every warning in JS
  (`point.character ?? indexAt(code, line, column)` — it already has `code` and `lineStarts`), so
  Rust never needs the composed text or its line table, and `code` itself is NOT sent back.
- `transforms[]` is omitted/empty for plain components (the common case — zero overhead).
- `start/end` of each transform are UTF-16 indices into the original source; `output` is the
  transformed text (needed for its line-start table); `mappings` is the raw VLQ string.
- Rust slices each transform's `input` from `source` via existing `utf16_byte`.
- `convert_warnings` gains the remap step before `utf16_byte`; `Warning`/`CompileResult` and every
  downstream consumer (linter, rules, callbacks) are unchanged.

### `bridge.cjs` edits

- Delete `sourceMapRemap`, the `decode` plumbing, the `embeddedSourceMapCodec` wrapper/fallback
  (lines 8-9), and the remap closures from both `transforms.push` sites.
- `run()` returns raw warnings + the transforms array. The region-chain/`maps`/`remap` block
  (252-272) moves to Rust.
- `applyCallbacks` is untouched (positions it receives are already final).
- `crates/oxvelte_compiler/src/lib.rs:155-173`: the bootstrap drops the
  `embeddedSourceMapCodec` wrapper and the `include_str!` of `vendor/sourcemap-codec.cjs`.

### Dev-only oracle + fixtures

`vendor/sourcemap-codec.cjs` remains in git, never embedded, never shipped (removed from
`.github/release-files.txt`), never executed by end users. It serves one purpose: generating the
golden fixtures that pin our Rust decoder to the exact JS semantics.

- New script `scripts/compiler-runtime/generate-sourcemap-fixtures.mjs` (dev tooling, not shipped):
  collects real `mappings` strings captured from sass, less, stylus, postcss, typescript
  (`transpileModule`), and babel transforms of representative inputs, runs each through the
  vendored `decode`, and writes JSON fixtures to `crates/oxvelte_compiler/tests/fixtures/`
  (committed; generation is reproducible but not part of CI — fixtures are the artifact).
- Rust decoder unit tests assert byte-for-byte equality with those fixtures, plus spec edge cases
  (empty lines, 1-field generated-only segments, 5-field named segments, multi-line maps, CRLF
  inputs, malformed-input errors).

## Phases

Each phase ends green: `cargo test --workspace`, `cargo clippy --workspace --all-targets
--features tsrs-inference,custom-rules` (0 warnings), `cargo fmt --check`.

**Phase 1 — Rust decoder, unused by the wire path yet.**
Add `sourcemap.rs` with `decode_mappings` + full unit tests (spec vectors + generated fixtures).
Generate fixtures with the oracle script. No behavior change.

**Phase 2 — remap port + protocol flip.**
Add `line_starts`/`SourceRemap`/region-chain/warning-remap to `sourcemap.rs` with unit tests
(ported cases: previous-line fallback, interpolation, CR vs CRLF, multi-transform ordering,
end-of-file tail position). Change `bridge.cjs` + `WireResult` + `convert_warnings` as above;
delete the codec from the bootstrap. Update `runtime.test.mjs`: remap assertions move out of the
JS tests (the bridge now returns raw data); the JS tests assert the new raw shape, and Rust
integration tests assert final remapped positions end-to-end through a real spawned runtime
(extending the pattern of the existing scss test at lib.rs:342 to less/stylus/postcss/ts/babel
and CRLF sources).

**Phase 3 — packaging + verification.**
- Remove `scripts/compiler-runtime/vendor/sourcemap-codec.cjs` from `.github/release-files.txt`
  (with a comment noting it is now dev-only) and update the comment block there.
- `THIRD_PARTY_NOTICES`: keep the @jridgewell/sourcemap-codec entry, amended to say the file is
  retained for development-time fixture generation only and is not embedded in the binary.
- Add a header comment to `vendor/sourcemap-codec.cjs` stating its dev-oracle role so nobody
  "reconnects" it.
- Full verification: workspace tests; `runtime.test.mjs` under both node and bun; the
  `upstream_parity` suite (1316 cases — must show zero new gaps); release build
  `cargo build --release --features custom-rules,tsrs-inference`; and a `strings`-style check that
  the compiled binary no longer contains the codec source (`grep -c "sourcemap-codec"`-style
  sentinel, e.g. the codec's distinctive function names, returns 0).
- Update `docs/research/dependency/rust_sourcemap_decode.md` status line to done.

## Risks & mitigations

- **Position-math regressions (the only real risk).** The fallback + interpolation semantics are
  parity-critical. Mitigations: fixtures pin the decoder to the JS oracle; the remap unit tests
  are ported case-by-case from bridge.cjs with the JS behavior as the specification; end-to-end
  Rust tests compile real preprocessed components through the spawned runtime; the 1316-case
  parity suite is the final gate. If anything disagrees, the phase-2 commit is self-contained and
  revertable (the codec file is still in the repo).
- **UTF-16 slicing mistakes in Rust.** All wire positions stay UTF-16 until the single existing
  `utf16_byte` boundary; slicing uses the already-tested `oxvelte_transport::span` helpers.
- **Response size.** `transforms[].output` duplicates transformed regions on the wire
  (kilobytes over a local pipe, only for transformed components) — negligible against the cost of
  the subprocess round-trip itself.
- **`runtime.test.mjs` drift.** It tests the bridge directly; its remap expectations move to Rust
  in the same commit so the two suites can't silently diverge.

## Out of scope (future work, noted only)

- Shrinking `bridge.cjs` further (preprocessor orchestration, config-graph tracking) — blocked on
  user npm packages, same as the bridge itself.
- `typescript-conditions.cjs` / `typescript-service.cjs` retirement — blocked on the JS-sidecar
  fallback for non-tsrs projects.
