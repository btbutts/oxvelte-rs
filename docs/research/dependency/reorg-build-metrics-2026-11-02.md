# Workspace reorg — build metrics (phase 7)

Measured 02NOV2026 on the dev Mac (Apple Silicon), after phases 1–6, per
`reorg-mods-to-crates.md` phase 7. Baselines are the pre-reorg numbers
recorded in that doc (same machine, warm dependency cache). Windows
numbers are outstanding — see the end.

## Headline numbers

| Metric | Pre-reorg (mono-crate) | Post-reorg (6 crates) | Verdict |
|---|---|---|---|
| Clean build of our code, deps warm (`cargo build`, both features) | 27.9s | 24–26s (3 runs) | ~equal, marginally better |
| Parallelism (user time ÷ wall time) | limited by 32 CGUs in one crate | ~13× (344s user / 26s wall) | better |
| Rebuild after editing one rule file | ~1s | ~5s | slightly slower (see notes) |
| Rebuild after editing `parser/template.rs` | ~1s (whole crate invalidated) | ~0.8s (only parser + relink) | equal or better |
| Feature flip cost (repeat) | full 50K-line recompile | **0** — both configurations stay cached | much better |
| `target/` size | 13 GiB | 15 GiB | +2 GiB (two feature configs × per-crate artifacts) |

## Feature-combination artifact reuse (the design goal)

Measured by building one feature configuration, flipping, and watching
which `oxvelte_*` crates recompile:

- **`tsrs-inference` flip:** only `oxvelte_linter` (where the feature
  lives), `oxvelte_tsrs` (newly enabled), and the root recompile.
  `oxvelte_parser`, `oxvelte_compiler`, `oxvelte_transport` are reused
  untouched. Exactly as designed.
- **`custom-rules` flip (first time):** `oxvelte_parser`,
  `oxvelte_compiler`, and `oxvelte_linter` also recompile;
  `oxvelte_transport` (pure-std, no external deps) never does.
  Mechanism undiagnosed after eliminating the obvious suspects —
  `serde`, `serde_json`, and `oxc` feature sets are verified identical
  with and without `custom-rules` (`cargo tree -e features`), so it is
  not ordinary feature unification on a shared dependency. boa_engine's
  presence in the graph appears to change Cargo's unit metadata for
  crates in the root's dependency closure. Not a reorg regression:
  pre-reorg, *any* flip recompiled the entire 50K-line crate. Worth a
  future look; `oxvelte_transport`'s immunity suggests the lever is
  dependency-closure overlap with boa's tree.
- **Repeat flips are free:** once both configurations have been built,
  toggling features rebuilds nothing (0.24s no-op). Cargo caches each
  feature-combination's artifacts concurrently. The pre-reorg pain
  (every flip = full recompile, every time) is gone.

## Clean-build wall time

Three controlled runs (`cargo clean -p` of all six members, then
`cargo build --workspace --features tsrs-inference,custom-rules`):
26s, 26s, 24s. The pre-reorg baseline was 27.9s for the single-crate
`cargo build --lib`. Wall time is a wash; the composition changed:
total CPU (user time) is higher (~344s vs the single crate's run)
because oxc generics monomorphize per consuming crate rather than once,
but it is spread across ~3× more parallel compilation units. On
many-core CI machines (Windows especially) the parallelism should win;
on few-core machines it may roughly tie. This matches the plan's
prediction that clean-build gains would be modest — the wins are
iteration, feature-combo reuse, and enforced boundaries.

## Incremental iteration

- Edit one rule file → rebuild `oxvelte_linter` + relink: ~5s (single
  sample; pre-reorg equivalent was ~1s). The regression is the thin-LTO
  relink of the root lib after the 27K-line linter crate rebuilds;
  still comfortably inside "edit → test" flow.
- Edit `parser/template.rs` → rebuild `oxvelte_parser` only +
  relink: ~0.8s. The linter's 27K lines are no longer invalidated by
  parser edits, and vice versa — the independent-fingerprint goal.

## `target/` size

13 GiB → 15 GiB. The increase is per-crate artifact directories and the
two cached feature configurations, not duplicated dependency builds
(oxc & co. are built once per configuration, shared by all members, as
before). As predicted in the plan, the reorg is not a `target/` diet;
it is a recompile-avoidance change.

## What was verified *not* to change

- Identical test totals every phase: 661 lib tests (now split 275 root
  + 2 compiler + 172 linter + 199 parser + 1 transport + 12 tsrs), all
  six integration suites, `upstream_parity` 1316 cases with no new
  gaps.
- Feature-off root build: zero `oxvelte_tsrs` in compile output and in
  `cargo tree`.
- Release profile untouched (`lto = true`, `codegen-units = 1`); the
  shipped artifact is one statically-linked binary, as before.

## Outstanding

- **Windows timings** — the motivating case. Repeat the clean-build
  series and the feature-flip test on a Windows machine and append
  results here.
- **`custom-rules` flip recompile mechanism** — see above.
- **Release-build timing** (`--release`, LTO fat): not measured here;
  expect ~equal (LTO dominates and is unchanged).
