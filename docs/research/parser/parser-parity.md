# Research: parser parity with `sveltejs/svelte`

Status: research complete (27OCT2026). All five divergences root-caused to
specific upstream PRs/releases and empirically verified against the real
`svelte@5.57.3` package; `Root.comments` and `Root.end` resolved as
harness-side fixes (see Decisions locked). Ready for implementation.
Related: `docs/research/test/pinned-rules.md` (Stage 1 landed the
`cargo get-svelte-fixtures` pipeline this document depends on).

## The problem

Our parser test fixtures (`fixtures/parser/{legacy,modern}/`, each sample an
`input.svelte` + `output.json` pair) were a hand-copy made in March 2026 by a
since-deleted `prepare.sh`. Stage 1 of the pinned-rules plan made them
refreshable from a pinned `sveltejs/svelte` release tag
(`OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"`).

Running the refresh exposed that **our fixture copy matches no released
svelte version**. Parser fixture test failures (`cargo test --lib`) by tag:

| svelte tag | failures (of 624 lib tests) |
|---|---|
| 5.57.3 (current pin) | 26 |
| 5.49.2 | 25 |
| 5.53.13 (mid-March 2026) | 23 |
| 5.40.0 | 73 |

Failures decrease toward newer releases but never reach zero, and moving
*older* makes it much worse. The inescapable conclusion: the March 2026
hand-copy was adjusted to match **our own serializer's** output conventions,
not upstream's — it is a snapshot of what our parser produces, not what
`sveltejs/svelte` produces.

**Therefore our Rust parser/serializer (`src/parser/`,
`src/parser/serialize.rs`) is genuinely diverging from the real
`sveltejs/svelte` parser**, and this workstream is about bringing it into
compliance with upstream's actual AST output at the pinned release. The
fixture pipeline is done and correct; what remains is parser work.

## Divergence inventory (measured 27OCT2026)

Method: extracted `packages/svelte/tests/parser-{legacy,modern}/samples/`
from the `svelte@5.57.3` release tarball into `/tmp/oxvelte-parser-parity/`
and structurally diffed every `output.json` against our fixtures (deep JSON
walk, key-order tolerant). No `input.svelte` differs. 25 of 112 shared
samples differ in `output.json`, and they bucket into exactly **5 root
causes**. Archaeology (below) then showed that **two of the five are
upstream test-harness artifacts, not real compiler behavior** — a major
simplification:

| # | Divergence | Upstream PR | First release | Real compiler change? |
|---|---|---|---|---|
| 1 | Modern `Root.comments` "missing" in fixtures | [#17671](https://github.com/sveltejs/svelte/pull/17671) | 5.53.0 | **No** — harness artifact (fixtures predate conditional normalization) |
| 2 | Modern `Root.end` excludes trailing whitespace | [#17125](https://github.com/sveltejs/svelte/pull/17125) | 5.45.8 | **No** (modern) — harness trims input before parsing. Legacy `html.end` trimming **is** real |
| 3 | Style/CSS `comments` arrays + `CSSComment` nodes | [#18637](https://github.com/sveltejs/svelte/pull/18637) | **5.56.9** | Yes |
| 4 | Pseudo-**element** `args` as `SelectorList` | [#18637](https://github.com/sveltejs/svelte/pull/18637) (same PR) | **5.56.9** | Yes — pseudo-elements only; pseudo-class `args` predate 5.53 |
| 5 | Each-block destructured context `loc` columns −1 | [#18738](https://github.com/sveltejs/svelte/pull/18738) | **5.57.1** | Yes (off-by-one *fix* upstream; no changelog entry) |

The five buckets in detail:

1. **Modern `Root.comments`** (16 modern samples differ). The real 5.57.3
   compiler still emits `Root.comments` (verified:
   `parse('<div></div>\n', { modern: true })` returns `comments: []`;
   parser initializes it at `phases/1-parse/index.js:101` and pushes tag
   comments from `state/element.js:527`). The fixture divergence is an
   artifact of upstream's test harness (`parser-modern/test.ts`): before
   PR #17671 it deleted `actual.comments` unconditionally, so old fixtures
   never contain the key; since #17671 (5.53.0, "feat: allow comments in
   tags") the deletion is conditional
   (`if (!expected.comments) delete actual.comments`). At 5.57.3 only
   `comment-in-tag` has `Root.comments` (with entries). #17671 also
   established the legacy split: legacy AST gets `_comments` on the `html`
   Fragment node, only when non-empty (renamed because Prettier chokes on
   `comments`). **Our serializer already matches the real compiler here.**
2. **`Root.end` and trailing whitespace** (~20 modern samples differ).
   Modern: since #17125 (5.45.8), `Root.start = 0` and
   `Root.end = template.length` — full source length, *untrimmed*
   (verified: 12-byte input → `root.end = 12`). The fixtures show one byte
   less because **both upstream harnesses trim the input before parsing**
   (`.replace(/\s+$/, '').replace(/\r/g, '')`). Our modern serializer
   (`serialize.rs:2379`, `source.len()`) is **already correct** — nothing
   to change in the modern serializer; the trim belongs in our fixture
   comparison path. Legacy **is** different: `legacy.js` computes the
   `html` Fragment span from first/last nodes and trims whitespace from
   both ends (verified: same 12-byte input → legacy `html.end = 11`) — our
   legacy path must replicate that trimming.
3. **Style/CSS `comments` + `CSSComment`** (legacy `css`,
   `whitespace-after-style-tag`; modern `css-nth-syntax`,
   `css-pseudo-classes`, `script-style-no-markup`,
   `semicolon-inside-quotes`). PR #18637 "fix: preserve CSS comments in
   the AST printer" (merged 2026-08-12, released **5.56.9**; the unmerged
   alternative was #18475). Semantics, verified empirically:
   - `StyleSheet` (modern css root) gets `comments: CSSComment[]` —
     **always present, empty when no comments**. Legacy `Style` inherits
     it via `{ ...node, type: 'Style' }` conversion; `parseCss()`'s
     `StyleSheetFile` also gets it.
   - Node shape: `{ type: 'CSSComment', value, start, end, position? }`.
     `value` is the inner text without `/* */`; `start`/`end` span the
     full comment. Optional `position` = character offset within a
     containing declaration value or at-rule prelude (used by upstream's
     `print` to re-insert inline comments); absent for structural
     comments.
   - Only `/* */` comments are captured (not `<!-- -->`).
   - Related but separate: #18611 (5.56.10), #18153 (5.55.6), #18553
     (5.56.10, inline `style=` attributes — unrelated).
4. **Pseudo-selector `args`** (modern `css-pseudo-classes`).
   Pseudo**Class**Selector (`:not`, `:nth-*`, `:is`, `:global`, …):
   unchanged — `args: SelectorList | null` (key always present) since well
   before 5.53; `:nth-*` An+B microsyntax as `Nth` nodes also predates
   5.53. The real change is Pseudo**Element**Selector
   (`::view-transition-old(x-y)`, `::highlight(name)`, …): at ≤5.56.8 the
   parser validated the parenthesized selectors and discarded them, with
   `end` stopping at the name; since #18637 (5.56.9) it attaches
   `args?: SelectorList` (key present **only when parens exist**,
   `...(args && { args })`) and `end` spans through the closing paren.
   Applies to all pseudo-elements with parens, no exception list. Our
   pseudo-class branch already emits `args` (`css.rs:286-308`); our
   pseudo-element branch discards them (`css.rs:282-285`).
5. **Each-block destructured context `loc` columns −1** (legacy
   `each-block-destructured`; modern `each-block-object-pattern`,
   `each-block-object-pattern-special-characters`, `loose-valid-each-as`).
   PR #18738 "perf: stop scanning the whole template for every expression"
   (merged 2026-08-31, released **5.57.1**, *no changeset/changelog
   entry*). The old `read_pattern` parsed a synthetic string (template
   prefix with non-newline chars blanked, one space removed to compensate
   for an inserted `(`) — byte offsets came out right but columns were +1
   (an upstream bug; the code comment even claimed columns were
   unaffected). The new code parses the real template prefix
   (`template.slice(0, i) + ' = 1'`) with `startLocation` passed to acorn,
   so columns are now true 0-based columns. Byte offsets unchanged.
   Scope: only destructuring patterns (`{...}`/`[...]` contexts of
   each/await blocks); identifier contexts and normal `{expr}` were
   always correct. Our `adjust_binding_columns`
   (`serialize.rs:1372-1396`) replicates the old upstream bug and must
   simply be removed.

### Inventory footnotes

- **Legacy `loose-invalid-expression`**: byte diff is JSON **key ordering
  only** — semantically identical; not actually failing.
- **Upstream-only samples (CREATE on next sync)**: legacy
  `convert-entities-line-feed`; modern `const-tag-parenthesized-init`,
  `css-nth-of-minified`, `loose-declaration-tag`, `parens`,
  `tag-type-identifier-probe-comment`.
- **Local-only samples (keep)**: `loose-unclosed-nested-at-eof` in both
  legacy and modern — ours, no upstream candidate.
- **`implicitly-closed-li-block` (legacy) is stale upstream**: the sample's
  `_config.js` has `skip: true`, so svelte's own suite never runs it, and
  the published 5.57.3 legacy parser actually throws `Unexpected block
  closing tag` on its input. Do not treat its `output.json` as ground
  truth; decide separately whether to keep syncing it.

## Fixture fidelity verification (done 27OCT2026)

Verified against the real published `svelte@5.57.3` compiler
(`/tmp/oxvelte-parser-parity/verify/verify.mjs`):

- Exact reproduction recipe (from upstream's own test drivers,
  `packages/svelte/tests/parser-{legacy,modern}/test.ts`):
  - Read `input.svelte`, strip ALL trailing whitespace and `\r` characters.
  - Legacy: `parse(input, { loose: dir.startsWith('loose-') })`.
  - Modern: `parse(input, { modern: true, loose: dir.startsWith('loose-') })`.
  - Round-trip via `JSON.parse(JSON.stringify(ast))`, serialize with
    `JSON.stringify(x, null, '\t') + '\n'`.
- Results: 110/112 shared samples byte-reproducible; the 2 exceptions
  (legacy `css`, `whitespace-after-style-tag`) differ only in the
  *position* of the root `"comments": []` key. All modern samples match
  after applying upstream's own `delete actual.comments` normalization.
- **Conclusion: the upstream committed fixtures are faithful ground
  truth** (except `implicitly-closed-li-block`, above).
- Serialization conventions our Rust serializer must mirror for
  byte-parity: tab indentation, trailing newline, and **keys in svelte's
  per-node construction order** (not alphabetical). Our test harness is
  key-order tolerant, so this matters only for byte-identical output.

## Impact analysis (done 27OCT2026)

Key finding: **the linter consumes the Rust `SvelteAst` structs and raw
source text, never the serialized JSON.** The changes are confined to
`src/parser/serialize.rs` + `src/parser/css.rs` + the fixture-comparison
path + fixture regeneration. No linter-rule changes are required. Edit map
(updated with archaeology verdicts):

| Change | Edit sites |
|---|---|
| (1) Root.comments | **Serializer unchanged** (keep `serialize.rs:2559` + collection `serialize.rs:2436-2558`). Harness-only: conditional `comments` removal in `run_modern_fixture` (`src/lib.rs:1234`) — see Decisions locked #1 |
| (2) Root.end | **Serializer unchanged** (`serialize.rs:2379` already correct). Harness-only: input trim in `run_legacy_fixture` (`src/lib.rs:1042`) and `run_modern_fixture` (`src/lib.rs:1222`) — see Decisions locked #2. Legacy: verify `html.end`/start trimming matches `legacy.js` (`serialize.rs:3026-3036`, `3758-3774`) — Decisions locked #3 |
| (3) CSS comments | Collect `/* */` spans in `allow_comment_or_whitespace` (`css.rs:944-989`), carry out via `CssParseResult` (`css.rs:52-59`), emit unconditional `comments: []` with `CSSComment { value, start, end }` (+ optional `position` for inline cases) in modern StyleSheet (`serialize.rs:2387-2399`) and legacy Style (`serialize.rs:3188-3209`); extend css.rs unit tests (`skips_css_and_html_comments` etc.) |
| (4) Pseudo-element args | Capture args at `css.rs:273-285` as `args?: SelectorList` (key only when parens exist; `end` spans through `)`); verify round-trip through `convert_css_to_modern` (`serialize.rs:2178-2201`) and legacy conversion (`css.rs:1169-1181`) |
| (5) Each-block columns | Delete `adjust_binding_columns` (`serialize.rs:1372-1396`) and its call sites (`serialize.rs:2820`, `4196`) — it replicates the pre-5.57.1 upstream bug |

Consumers checked and unaffected: all `src/linter/rules/*` (they read
`SvelteAst` spans/text), `src/custom_rules.rs` (serde of Rust structs),
`src/compiler.rs` (Node bridge to the real compiler), `src/main.rs` `parse`
CLI (pass-through), `tests/parser_lint_contract.rs` (asserts on LetTag
internals only), `tests/upstream_parity.rs` / `upstream_adapter.rs` /
`jobsys_regressions.rs` (lint-diagnostic parity only). The fixture harness
(`src/lib.rs:995-1062` legacy, `1177-1284` modern) compares structurally:
key-order tolerant, strict on key sets and scalar values — so fixtures must
be regenerated in lockstep with serializer changes.

## Decisions locked (27OCT2026)

1. **`Root.comments`: match the compiler, normalize in the harness
   (Option A).** Keep emitting modern `Root.comments` exactly as today —
   it is real AST that matches the 5.57.3 compiler. Teach our fixture
   harness upstream's own conditional rule instead. **Edit site:**
   `src/lib.rs` `run_modern_fixture` (~line 1234), immediately before the
   `json_diff` call:

   ```rust
   // Mirror upstream's parser-modern harness: fixtures committed before
   // svelte 5.53 have no root `comments` key, so drop it before comparing.
   if expected.get("comments").is_none() {
       actual.as_object_mut().map(|m| m.remove("comments"));
   }
   ```

   When a fixture *does* carry `comments` (today: `comment-in-tag`), the
   entries are compared for real. Legacy needs nothing — `_comments`
   only-when-non-empty already matches.
2. **`Root.end`: trim the input in the harness (Option A).** The modern
   serializer stays as-is (`Root.end = source.len()` matches the real
   compiler). Both fixture harnesses replicate upstream's
   `.replace(/\s+$/, '').replace(/\r/g, '')` input normalization before
   parsing. **Edit sites:** `src/lib.rs` `run_legacy_fixture` (~line 1042)
   and `run_modern_fixture` (~line 1222), right after reading
   `input.svelte`:

   ```rust
   // Mirror upstream's harness: fixtures describe the input with trailing
   // whitespace and carriage returns stripped.
   let input = input.trim_end().replace('\r', "");
   ```

   The trimmed string feeds both `parser::parse(...)` and the serializer.
3. **Legacy span verification pass.** Confirm our legacy `html` start/end
   whitespace trimming (`src/parser/serialize.rs:3026-3036`,
   `3758-3774`) matches svelte's `legacy.js` semantics (trim leading and
   trailing whitespace off the fragment span) across the legacy fixtures.
   Expected to be a recorded confirmation with no code change.
4. **Bundled commit sequencing.** Decisions 1-2 turn the parser fixture
   tests **red against the current old fixtures** (the hand-copy has the
   unfiltered semantics baked in). Therefore the harness changes, the real
   serializer work (changes 3/4/5), and the `cargo get-svelte-fixtures`
   re-sync must land **together in one commit**. Nothing ships
   harness-only.

## Dependency upgrade: cssparser 0.38 / selectors 0.41 (probed 27OCT2026)

Current pins: `cssparser = "0.36"`, `selectors = "0.37.0"`. Targets:
`cssparser = "0.38.0"`, `selectors = "0.41.0"`. Both are Servo's typed CSS
machinery, used only by `src/parser/selector.rs` (the typed selector AST
behind `consistent_selector_style`), `src/linter/rules/no_unused_class_name.rs`
(raw cssparser token walk), and `src/linter/rules/consistent_selector_style.rs`
+ `src/linter/rules/indent/mod.rs` (`selectors::matches`). This is **not** the
hand-rolled CSS parser in `src/parser/css.rs` that feeds the serializer — so
the upgrade has zero effect on fixture output and **can land as its own
commit, before or after the bundled parity commit**.

A throwaway probe (bump versions, `cargo check`, revert — nothing committed)
produced exactly 22 errors in 2 files, bucketing into 6 breaking changes:

1. **`cssparser::ParserInput` removed.** `cssparser::Parser::new` now takes
   `&str` directly (`Parser::new(input: &'i str)`). Fix: delete the
   `ParserInput` import/usage at `src/parser/selector.rs:17` and
   `src/linter/rules/no_unused_class_name.rs:107`, pass the source string
   straight to `Parser::new`.
2. **`Parser<'i, 't>` → `Parser<'i>`** (one lifetime). Mechanical signature
   updates wherever we name `CssParser<'i, 't>` (`selector.rs:175, 205, 270,
   285, 311, 321`; `no_unused_class_name.rs:118`).
3. **`ParseError<'i, E>` → `ParseError<E>`** (no lifetime). Mechanical:
   drop the `'i` from every `cssparser::ParseError<'i, Self::Error>`
   annotation (`selector.rs:168, 177, 188, 198, 206, 211`;
   `no_unused_class_name.rs:135`).
4. **`SelectorParseErrorKind<'i>` → `SelectorParseErrorKind`** (no
   lifetime). One site: `selector.rs:143` (`type Error = ...`).
5. **`NonTSPseudoClass::Impl` / `PseudoElement::Impl` associated types
   removed** from those traits (the link back to `SelectorImpl` now lives
   elsewhere — `SelectorVisitor`-side). Fix: delete `type Impl = OxSelector;`
   from the impls at `selector.rs:100` and `selector.rs:120`. Our
   `SelectorImpl` impl (`selector.rs:126-137`) is unchanged.
6. **`parse_non_ts_pseudo_class` / `parse_pseudo_element` lose the
   `SourceLocation` first parameter** (now `(&self, name: CowRcStr<'i>)`).
   Fix: drop the `_location: SourceLocation` param at `selector.rs:165` and
   `selector.rs:195` (and the now-unused `SourceLocation` import). The
   functional variants (`parse_non_ts_functional_pseudo_class`,
   `parse_functional_pseudo_element`) keep their shapes, minus the lifetime
   changes above.

Lockfile side effects observed during the probe (all benign): the `phf`
0.13 family drops out entirely, and `servo_arc` bumps 0.4.3 → 0.5.0.

Not breaking, but worth noting: selectors 0.41's `Parser` trait grew new
capability flags (`parse_nth_child_of`, `parse_parent_selector`, etc.). We
already opt into the ones we need (`parse_is_and_where`, `parse_has`,
`parse_part`, `parse_slotted`, `parse_host`, `parse_nth_child_of` at
`selector.rs:145-162`); no behavior change expected from the defaults.

Validation after the bump: `cargo check`, `cargo clippy`, `cargo test --lib`
(the typed-selector path is exercised by `consistent_selector_style` and
`no_unused_class_name` fixtures in the linter suite), plus
`cargo test --features custom-rules` for the full matrix.

## Open decisions (for the user)

1. **Staging of capability work.** Changes (3) and (4) add parser
   *capability* (CSS comment collection, pseudo-element arg trees);
   change (5) is a deletion of `adjust_binding_columns`. All must land in
   the bundled commit per decision 4 above, but the *work* can still be
   built and reviewed in stages. Order of attack within the workstream?
2. **Stale upstream sample.** Keep syncing `implicitly-closed-li-block`
   (whose upstream output cannot be reproduced by its own compiler), or
   carry it as a local-only sample?

## Interim state (until this lands)

- `fixtures/parser/` remains at the old hand-copy; `cargo test --lib` is
  green (625 passing).
- `OXVELTE_FIXTURES_SVELTE_TAG` stays pinned at `5.57.3` in
  `.cargo/config.toml`, but **`cargo get-svelte-fixtures` must not be
  re-run (and its output must not be committed) until this workstream
  lands** — running it turns the parser fixture tests red, by design.
- Once the parser matches upstream at the pin, re-run
  `cargo get-svelte-fixtures`, review the `UPDATE`/`CREATE`/`SKIP` report,
  and commit fixtures and parser changes together.
