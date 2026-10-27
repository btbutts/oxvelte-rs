# Research: parser parity with `sveltejs/svelte`

Status: problem identified (27OCT2026); the research below is defined but not
yet done. Related: `docs/research/test/pinned-rules.md` (Stage 1 landed the
`cargo get-svelte-fixtures` pipeline this document depends on).

## The problem

Our parser test fixtures (`fixtures/parser/{legacy,modern}/`, each sample an
`input.svelte` + `output.json` pair) were a hand-copy made in March 2026 by a
since-deleted `prepare.sh`. Stage 1 of the pinned-rules plan made them
refreshable from a pinned `sveltejs/svelte` release tag
(`OXVELTE_FIXTURES_SVELTE_TAG = "5.57.3"`).

Running the refresh exposed that **our fixture copy matches no released svelte
version**. Parser fixture test failures (`cargo test --lib`) by tag:

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

## Observed divergences at 5.57.3 (from the fixture diffs)

Each item below was seen in the `git diff` of `fixtures/parser/` after syncing
against `svelte@5.57.3`:

1. **Modern `Root.comments` removed.** Upstream no longer emits a `comments`
   field on the modern `Root` node (even when comments exist, e.g.
   `comment-before-function-binding`); we always emit it.
2. **Modern `Root.end` semantics.** Upstream's `Root.end` is one byte smaller
   than ours on inputs ending in a trailing newline (e.g. 18 → 17 on
   `if-block`) — we appear to include trailing whitespace, upstream does not.
3. **Legacy `Style.comments` added.** Legacy-style nodes gained a `comments`
   array (empty in `css`, `whitespace-after-style-tag`); we emit none.
4. **Modern CSS comments collected.** `css-nth-syntax` gained `CSSComment`
   entries inside the CSS node's `comments` array; we collect none.
5. **Pseudo-class arguments parsed.** `css-pseudo-classes` shows `:not()` /
   `:nth-*()` arguments as real `SelectorList` nodes upstream; ours are absent
   or unstructured.
6. **Each-block expression columns shifted by 1.**
   `each-block-object-pattern(-special-characters)` expression `loc` columns
   are one lower upstream (e.g. 19 → 18) — an expression-offset base change.
7. **Legacy loose parsing restructured.** `loose-invalid-expression` grew a
   `Fragment` wrapper with `Element`/`Attribute` children upstream — a
   structural change, not a field tweak.

## Research to do (not yet done)

- **Locate each change upstream.** For every divergence above, find the
  sveltejs/svelte PR and release that introduced it (changelog archaeology
  between ~5.40 and 5.57.3) and record the exact new semantics: Is
  `Root.end` "end of last meaningful token" or "input length minus trailing
  whitespace"? Were modern `Root.comments` moved into the `Fragment` or
  dropped entirely? What is the `CSSComment` node's full shape?
- **Verify fixture fidelity.** Run the real svelte 5.57.3 compiler
  (`svelte/compiler`, modern and legacy modes) over each fixture's
  `input.svelte` and confirm the committed `output.json` is verbatim compiler
  output, so we know we are targeting ground truth. (Expected: yes.)
- **Impact analysis on the linter.** Enumerate which `src/linter/rules/*` and
  other consumers read the fields whose shapes change (`Root.end`,
  `comments` attachment, CSS comments, pseudo-class args). AST shape changes
  may ripple into rule behavior, not just serializer output.
- **Scope decision.** Decide whether we target exact 5.57.3 output including
  the *added* structure (CSSComment nodes, SelectorList args — new parser
  capability, not just serializer changes), or stage it: serializer-level
  field/offset compliance first, new node types second.
- **Legacy vs. modern sequencing.** The legacy serializer
  (`src/parser/serialize.rs`) and modern serializer change independently;
  decide whether they land together or as separate passes.

## Interim state (until this lands)

- `fixtures/parser/` remains at the old hand-copy; `cargo test --lib` is
  green (624 passing).
- `OXVELTE_FIXTURES_SVELTE_TAG` stays pinned at `5.57.3` in
  `.cargo/config.toml`, but **`cargo get-svelte-fixtures` must not be re-run
  (and its output must not be committed) until this workstream lands** —
  running it turns the parser fixture tests red, by design.
- Once the parser matches upstream at the pin, re-run
  `cargo get-svelte-fixtures`, review the `UPDATE` report, and commit fixtures
  and parser changes together.
