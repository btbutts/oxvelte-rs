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
  **Mechanism diagnosed 02NOV2026:** `boa_engine` changes the feature
  sets of several shared transitive dependencies when it enters the
  graph. Confirmed at the fingerprint level
  (`target/debug/.fingerprint/*/lib-*.json`):
  - `serde` gains **`rc`** (boa requires it) — serde rebuilds, and so
    does its whole downstream closure (`serde_json`, `oxc`, `cssparser`,
    `selectors`, then our three crates).
  - `syn` gains `fold`/`visit-mut` and `quote`/`proc-macro2` gain
    `default` — but **only in the host (build-dependency) context**.
    serde_derive, clap_derive, oxc_ast_macros, miette-derive,
    thiserror-impl, and boa_macros are proc-macro crates, so the
    proc-macro ecosystem resolves features in the host context, which
    resolver 2 unifies separately from the normal context.
  - Smaller flips further down (`libc/extra_traits`, `hashbrown`,
    `bitflags/serde`, …).
- **Decision (02NOV2026): accept the first-flip rebuilds; no feature
  pins.** A `serde/rc` pin was tried and does work for the serde chain,
  but it does not stop the host-context proc-macro cascade, so it was
  reverted — no reason to carry an always-on feature that only half
  mitigates. Pinning boa's full transitive footprint is not viable
  either: pins from the root do not reliably reach the host resolution
  context, and the footprint can change on any boa upgrade.
  `cargo tree -e features` had masked all of this because the
  dev-dependency self-reference keeps the dev-context features enabled
  in both views; the fingerprint JSONs are the ground truth.
- **Why it is acceptable:** the cascade runs once per feature
  configuration; both configurations then stay cached and repeat flips
  cost nothing (measured: 0.24s no-op). Pre-reorg, every flip rebuilt
  the entire 50K-line crate every time. If the residual first-flip
  cost ever becomes annoying, the structural fix is to make
  `custom-rules` a default feature (dev, test, and release all build
  one configuration anyway; release.yml already ships it on).
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
- **Release-build timing** (`--release`, LTO fat): not measured here;
  expect ~equal (LTO dominates and is unchanged).
