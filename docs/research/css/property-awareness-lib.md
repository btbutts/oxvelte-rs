# Enable robust css property awareness

### Preface:
There is an existing npm package `@mdn/browser-compat-data` which already has an extensive catalogue of CSS properties.
It is used by other open source projects like [nikolai-shabalin/css-properties](https://nikolai-shabalin.github.io/css-properties/en/) for the sole purpose of identifying css props for that identification and docs search site.

However, no single upstream catalogue contains every property in our manually maintained `src/linter/rules/known_css_properties_0_37_0.json` (e.g. `-khtml-*`, additional `-webkit-*` / `-moz-*` props). Therefore, rather than depending on one source, we will merge, condense, and dedupe **six** upstream catalogues into a single generated `known_css_properties.json` (see 'Data sources' below). Overlapping coverage is a feature: if one repo removes or misses a property, the others still cover it.

**Sourcing decision (23OCT2026): all six sources are fetched as GitHub repo archive tarballs — no npm, no `node_modules/`.** Each repo is pinned to an exact commit hash kept in `.cargo/config.toml`; bumping a hash is how we pull a newer snapshot. This keeps the build independent of any local npm state (which may disappear entirely in future repo cleanup) and makes generation hermetic and reproducible.

### Goal:
Rather than manually maintaining a json list of these props, via the upstream's `src/linter/rules/known_css_properties_0_37_0.json`, this is something we can generate programatically.
In fact, we shouldn't need to create this file at all, as we can leverage a `build.rs` explicitly defined in our `Cargo.toml` that when triggered via our standard `cargo build --release` will:
1. Look for a previously generated, cached copy of `known_css_properties.json` in a staging dir we maintain (see 'Cache / staging dir' below). If found, copy it into cargo's `OUT_DIR` and stop.
2. Otherwise, commence generation immediately: download each of the six source repo tarballs from GitHub at their pinned commits into a throwaway `tmp/` dir, extract only the json file(s) we care about from each, merge, dedupe, strip dead `-ms-*` entries, sort, and write the resulting `known_css_properties.json` to **both** `resources/css/` (the cache) and `$OUT_DIR/`.

The generated json keeps the exact schema of today's vendored file — `{ "properties": ["accent-color", ...] }` — so `src/linter/rules/no_unknown_style_directive_property.rs`'s `KnownCssProperties` struct does not change; only its `include_str!` path changes from the vendored copy to the generated one:

```rust

fn known_css_properties() -> &'static FxHashSet<String> {
    static PROPERTIES: OnceLock<FxHashSet<String>> = OnceLock::new();
    PROPERTIES.get_or_init(|| {
        let data: KnownCssProperties = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/known_css_properties.json"
        )))
        .expect("Generated known_css_properties.json data should be valid JSON");

        data.properties.into_iter().collect()
    })
}

```

### Data sources:
Six repositories, fetched at generation time and merged with dedup. Schemas verified 23OCT2026 against the raw files; tarball availability verified 23OCT2026 against `codeload.github.com` (see the WebKit exception below the table):

| # | Repo | Path inside repo | Format | Tarball member(s) to extract |
|---|------|------------------|--------|------------------------------|
| 1 | `known-css/known-css-properties` | `data/all.json` | `{ "properties": ["-epub-...", ...] }` | single file. Origin of our current vendored file; the only source carrying legacy prefixes (`-khtml-*`, `-epub-*`, `-wap-*`). Our output schema matches this file's. |
| 2 | `mdn/browser-compat-data` | `css/properties/*.json` | one file per property; the **filename** (minus `.json`) is the property name | **no extraction needed** — stream the tarball's entry list and collect member names matching `css/properties/*.json`. |
| 3 | `csstree/csstree` | `data/patch.json` | root `properties` key is an **object keyed by property name** (85 entries, mostly `-moz-*` patches) | single file; ignore sibling `atrules` / `types` keys. |
| 4 | `mdn/data` | `css/properties.json` | single object whose **top-level keys** are property names (800+) | single file; skip the `"--*"` custom-property wildcard. Carries many `-ms-*` props — strip per merge rules. May be superseded by #6 long-term. |
| 5 | `WebKit/WebKit` | `Source/WebCore/css/CSSProperties.json` | root `properties` key is an **object keyed by property name** (632 entries; 69 `-webkit-*`) | ⚠️ **tarball unavailable — see exception below.** Skip meta keys (`instructions`, `descriptors`, `categories`, `shared-grammar-rules`). |
| 6 | `w3c/webref` | `ed/css/*.json` | per-spec files, each `{ "spec": {...}, "properties": [ { "name": "margin-top", ... } ] }` — names live at `properties[].name`, **not** as keys | extract all members matching `ed/css/*.json` (per-spec layout verified 23OCT2026: `ed/css/CSS.json` alone covers only CSS Level 2). |

**WebKit exception (verified 23OCT2026):** GitHub refuses on-demand archive generation for `WebKit/WebKit` — both `https://github.com/WebKit/WebKit/archive/<ref>.tar.gz` and the `codeload.github.com` equivalent return **HTTP 422** for branch names and commit hashes alike (repo too large). The other five repos return 200. Source #5 is therefore fetched as a **single raw-file GET** at the pinned commit: `https://raw.githubusercontent.com/WebKit/WebKit/${COMMIT}/Source/WebCore/css/CSSProperties.json` (verified working). The per-source entry in `css_prop_sources` (see 'build.rs architecture') carries a fetch-strategy marker — `Tarball` vs `RawFile(path)` — and WebKit is the only `RawFile` source today.

Merge rules:
- **Strip `-ms-*` prefixed properties unconditionally**, regardless of source — they target dead Internet Explorer and pre-Chromium Edge.
- Keep other vendor prefixes (`-webkit-*`, `-moz-*`, `-khtml-*`, `-epub-*`, `-wap-*`, `-o-*`): the rule already unprefixed-matches via `vendor_prefix()` in `no_unknown_style_directive_property.rs`, and legacy-prefixed content still appears in real templates.
- Skip non-property meta entries (`--*`, `instructions`, etc.).
- Dedupe case-insensitively (CSS property names are ASCII case-insensitive), emit lowercase, sort alphabetically.
- Output: `{ "properties": [ ... ] }`, one array, matching the current vendored schema.

### Pinned commits (`.cargo/config.toml`):
The repo already ships a committed `.cargo/config.toml` with an `[env]` table using the `OXVELTE_` prefix, so the six commit hashes follow that convention. `[env]` values are visible to build scripts, and values already present in the environment take precedence (unless `force` is set), so CI and ad-hoc overrides still win:

```toml
# .cargo/config.toml  (append to the existing [env] table)
# Pinned upstream snapshots for build.rs's CSS property generation
# (see docs/research/css/property-awareness-lib.md). Bump a hash to
# pull a newer snapshot, then delete resources/css/known_css_properties.json
# to regenerate.
OXVELTE_CSS_SRC_KNOWN_CSS_PROPERTIES_COMMIT = "d8007804a12fa5431a2d8e3b55252052442e51d0"  # 2026-09-02
OXVELTE_CSS_SRC_MDN_BROWSER_COMPAT_DATA_COMMIT = "6ced300dbd4cb9dbca68b2c2dffb8627d2aac82f" # 2026-10-22
OXVELTE_CSS_SRC_CSSTREE_COMMIT = "f8980154f79110833bf56c391cd45fe5f9205752"                 # 2026-09-16
OXVELTE_CSS_SRC_MDN_DATA_COMMIT = "fc60c55f5018ea399516a84ea2c64adf7731da82"                # 2026-10-15
OXVELTE_CSS_SRC_WEBKIT_COMMIT = "21bb1f66c37c7a8380cf7597d647eecb87f55ebd"                  # 2026-10-23
OXVELTE_CSS_SRC_W3C_WEBREF_COMMIT = "64111c5ac4fccf60f45bc266a322ea3fad8098f4"              # 2026-10-23
```

(Hashes above are each repo's HEAD as of 23OCT2026 — refresh them at implementation time.) `build.rs` should also emit `cargo:rerun-if-env-changed=<VAR>` for each of the six so that bumping a hash re-triggers the build script.

### build.rs architecture:
The generation logic is driven by a single loop over a source table, with one generic downloader helper:

**1. `fetch_github_repo(owner, name, commit_hash, archive_type)` — a dumb, generic downloader.** It contains *no* hardcoded URL elements of its own; everything is supplied by the caller, so it can download any GitHub repo archive and be reused for other purposes later. It formulates `https://github.com/${owner}/${name}/archive/${commitHash}${ext}` where `archive_type` selects the suffix: `tarball` → `.tar.gz`, `zip` → `.zip`. Only `tarball` is used initially, but the `zip` arm is implemented from the start (URL suffix + zip extraction path). The function performs the HTTPS download into a throwaway `tmp/` dir and returns the archive path (or bytes) to the caller — nothing else.

**2. `css_prop_sources` — a HashMap (dictionary) of the six sources.** Each entry holds the URL ingredients *except* the commit hash, which is populated from the `.cargo/config.toml` env vars at runtime, plus the per-source handling strategy:

```text
css_prop_sources = {
  "known-css-properties":   { owner: "known-css", name: "known-css-properties",
                              archive: Tarball, commit: env(OXVELTE_CSS_SRC_KNOWN_CSS_PROPERTIES_COMMIT),
                              extract: SingleFile("data/all.json"),            schema: PropertiesArray },
  "mdn-browser-compat-data":{ owner: "mdn",       name: "browser-compat-data",
                              archive: Tarball, commit: env(OXVELTE_CSS_SRC_MDN_BROWSER_COMPAT_DATA_COMMIT),
                              extract: ListMembers("css/properties/*.json"),   schema: FileNames },
  "csstree":                { owner: "csstree",   name: "csstree",
                              archive: Tarball, commit: env(OXVELTE_CSS_SRC_CSSTREE_COMMIT),
                              extract: SingleFile("data/patch.json"),          schema: ObjectKeys("properties") },
  "mdn-data":               { owner: "mdn",       name: "data",
                              archive: Tarball, commit: env(OXVELTE_CSS_SRC_MDN_DATA_COMMIT),
                              extract: SingleFile("css/properties.json"),      schema: ObjectKeys(top_level) },
  "webkit":                 { owner: "WebKit",    name: "WebKit",
                              archive: RawFile("Source/WebCore/css/CSSProperties.json"),  # tarballs 422 — see exception
                              commit: env(OXVELTE_CSS_SRC_WEBKIT_COMMIT),     schema: ObjectKeys("properties") },
  "w3c-webref":             { owner: "w3c",       name: "webref",
                              archive: Tarball, commit: env(OXVELTE_CSS_SRC_W3C_WEBREF_COMMIT),
                              extract: MultiFile("ed/css/*.json"),             schema: NamedObjects("properties[].name") },
}
```

**3. A single `for` loop drives the main sequence.** For each entry in `css_prop_sources`: populate its commit hash from the env var → call `fetch_github_repo(...)` (or the raw-file GET for the WebKit entry) → run that source's extraction rule against the archive in `tmp/` (single-file read, member-name listing, or multi-file glob per the table) → parse per its schema and feed the property names into one shared deduping set (e.g. a `BTreeSet<String>`, which also gives us sorted output for free) → delete the source's archive from `tmp/` and move to the next source. One source is fully processed before the next begins, deduping as we go — no intermediate per-source lists, keeping the logic minimal.

**4. After the loop:** apply the merge rules (strip `-ms-*`, lowercase, meta-key skips — cheap to do per-insert or as a final filter), serialize `{ "properties": [...] }`, and write it to **both** `resources/css/known_css_properties.json` (cache; create the dir on demand) and `$OUT_DIR/known_css_properties.json`.

**5. Error policy (per user direction, 23OCT2026):**
- **Download failure of any single source → warn and continue.** Emit `cargo::warning=...` naming the source and the likely reason (timeout, HTTP 404/422 for a bad or unavailable commit, DNS/network, etc.), with a catch-all warning for anything else — then proceed with the remaining sources. The merged list is deliberately redundant across sources, so a partial fetch still yields excellent coverage.
- **Extraction/parse failure once a tarball is in hand → fail the build.** If our member paths or schemas no longer match, the upstream repo restructured and every source will likely fail the same way — that is a code problem, not a network problem, and it should be loud.
- **All six downloads failed (empty set) → fail the build.** Shipping an empty property list would turn `no-unknown-style-directive-property` into a false-positive machine; warn-and-continue must never produce an empty output file.

**6. Housekeeping:** `build = "build.rs"` and a minimal `[build-dependencies]` set in `Cargo.toml` (small sync HTTP client e.g. `ureq`, `serde_json`, plus `tar`+`flate2` and `zip` for extraction — pure-Rust extraction keeps this portable and avoids shelling out to a system `tar`). Emit `cargo:rerun-if-changed=resources/css/known_css_properties.json` and the six `cargo:rerun-if-env-changed=` lines so the script re-runs only when the cache or a pin changes — never unconditionally per build.

### Cache / staging dir:
**Decision: `resources/css/known_css_properties.json`**, with a single entry added to the repo's **root** `.gitignore`:

```
# Generated by build.rs (see docs/research/css/property-awareness-lib.md)
/resources/css/known_css_properties.json
```

Rationale vs. the locally scoped `resources/css/.gitignore` alternative: ignoring just the generated file (not the whole directory) in the root `.gitignore` avoids committing a near-empty directory whose only file is a `.gitignore` — which reads as dead weight to anyone browsing the repo, and would need a comment block to explain itself. The root `.gitignore` already documents *why* each entry exists, so the intent stays discoverable in one conventional place. `build.rs` creates `resources/css/` on demand (git never tracks empty dirs, so fresh clones simply won't have it until the first generation).

The `tmp/` download dir is likewise transient: archives are deleted per-source inside the loop, and the dir itself can live under the system temp dir or `target/` so it never needs its own ignore rule.

### Implementation checklist:
1. **`Cargo.toml`** — add `build = "build.rs"` and `[build-dependencies]` per 'build.rs architecture' §6.
2. **`.cargo/config.toml`** — append the six `OXVELTE_CSS_SRC_*_COMMIT` vars to `[env]` (hashes refreshed to HEAD at implementation time).
3. **`.gitignore`** — add `/resources/css/known_css_properties.json`.
4. **`build.rs`** — implement `fetch_github_repo`, `css_prop_sources`, the driving `for` loop, merge rules, dual output write, and the error policy above.
5. **`src/linter/rules/no_unknown_style_directive_property.rs`** — change only the `include_str!` path per the 'Goal' example; delete `src/linter/rules/known_css_properties_0_37_0.json`.
6. **`.github/workflows/source-release.yml`** — the injection changes in 'Release workflow changes' below.
7. **Refresh policy** — bump a hash in `.cargo/config.toml`, delete `resources/css/known_css_properties.json`, rebuild. A convenience script (`scripts/refresh-css-properties.sh` doing exactly that) is optional but cheap.

### Planning (fetch mechanics):
All six sources come from GitHub directly — no npm, no GitHub API directory listings (which are rate-limited to 60 unauthenticated reqs/hour), no `node_modules/`. The archive URL pattern is uniform and was tested against multiple repos with arbitrary commit hashes:

`https://github.com/${owner}/${repo}/archive/${commitHash}.tar.gz` (or `.zip`)

which redirects to `codeload.github.com`. `raw.githubusercontent.com` (used for the WebKit exception) and `codeload.github.com` do **not** count against the API rate limit, so the whole generation run needs no token. Because we extract only the member(s) we need — or, for `mdn/browser-compat-data`, merely read the entry *list* of `css/properties/*.json` without extracting file contents at all — even the larger tarballs (BCD, webref) cost bandwidth but almost no disk or CPU.

> NOTE:
>
> The archive URL templates, preserved from earlier planning:
> `https://github.com/${owner}/${repo}/archive/${commitHash}.zip` OR
> `https://github.com/${owner}/${repo}/archive/${commitHash}.tar.gz`
>
> Then, similar to what has been discussed within `docs/research/test/pinned-rules.md`, we need only extract the file(s) we need from the tarball version.

### Release workflow changes (`.github/workflows/source-release.yml`):
The tarball continues to be issued **from the tag** — `git archive --format=tar "$TAG"` over the paths in `.github/release-files.txt` — never from HEAD or the working tree. That is precisely why the generated json needs special handling: it is gitignored, so it can never come out of `git archive`. The workflow must inject it into the staged tree instead:

1. Add a step **before** 'Build the tarball' that runs the generator on the release runner (e.g. `cargo build --release` once, or a direct generator invocation), producing `resources/css/known_css_properties.json` in the working tree. The runner has network access, so all six sources are reachable.
2. In 'Build the tarball', **after** `git archive ... | tar -x -C "$stage"`, copy the generated file into place: `cp resources/css/known_css_properties.json "$stage/${name}/resources/css/"`. Everything else in the tarball still comes from the tag untouched. (`.cargo/config.toml` is tracked, so the pinned hashes travel with the tag — the generator step above is what needs the network, not the tarball build.)
3. Add `resources/css/known_css_properties.json` to the non-negotiable `shipped.txt` presence checks alongside `Cargo.toml`, `Cargo.lock`, etc. Also confirm `.cargo/config.toml` is covered by the `release-files.txt` paths or add it — without it the tarball build still works (cache present), but keeping the pins with the source keeps the release self-describing.
4. The existing 'Smoke-test the tarball' step (`cargo build --release --locked` on the extracted tarball) then exercises the offline path: `build.rs` finds the shipped cache and never touches the network — which is what lets package managers like homebrew build from the minimal tarball with no additional web requests or deps.

`.github/release-files.txt` does **not** gain an entry for the json — that list feeds `git archive`, which can only emit tracked files, and the whole point is that this file is untracked. The injection in step 2 is the mechanism.

### Other Info:
To ensure we can provide minimal release tarballs of the project source code, so that we don't have to include static copies of any of the six upstream repos, `build.rs` pulls from a cached copy of the generated `known_css_properties.json` if it exists in the build env context. To be clear, `known_css_properties.json` is excluded from the repo via `.gitignore`, but during source-release tarball creation the workflow generates it (network available there) and injects it into the staged tarball tree as described in 'Release workflow changes'. Thus the compiled releases embed their copy of `known_css_properties.json` via `include_str!` from `OUT_DIR`, and tarball builds find the cache at `resources/css/known_css_properties.json` and embed it the same way — identical output, no network required.

### Final Thoughts 21OCT2026:
~~It is possible that `mdn/browser-compat-data` may not contain every css property that is already defined insided the manually maintained `src/linter/rules/known_css_properties_0_37_0.json`. Therefore, we may find other npm packages we can source these props from, but its also possible that a mix of #1 and #2 from 'Planning' above may be necessary to make this approach viable, hence another reason why locally cacheing `known_css_properties.json`, and NOT only relying on `cargo`'s build context is beneficial.~~

### Final Thoughts 23OCT2026:
The coverage concern from 21OCT2026 is resolved by the six-source merge in 'Data sources': with `known-css/known-css-properties`, `mdn/browser-compat-data`, `csstree/csstree`, `mdn/data`, `WebKit/WebKit`, and `w3c/webref` all contributing, as soon as any one of them adds a new property we learn of it on the next regeneration, and no single repo's removal decision can shrink our list. Schemas for all six sources were verified against the live files on 23OCT2026 (including the correction that webref's `ed/css/` is per-spec, and that WebKit currently carries 69 `-webkit-*` props and zero `-khtml-*` — legacy prefixes come from source #1).

Sourcing was then simplified to tarball-only: no npm method for any source, commits pinned in `.cargo/config.toml`, one generic `fetch_github_repo(owner, name, commit, archive_type)` helper, one `css_prop_sources` HashMap, one driving `for` loop that fetches → extracts → dedupes one source at a time. Live verification on 23OCT2026 produced two findings that shaped the design: (a) GitHub returns **HTTP 422 for all `WebKit/WebKit` archive URLs**, so source #5 uses a raw-file GET at the pinned commit instead — the only deviation from the uniform tarball pattern; (b) the other five repos serve tarballs fine (HTTP 200 spot-checks). Download failures warn-and-continue per source (`cargo::warning=`), extraction/parse failures fail the build, and an all-sources-failed empty result fails the build as a false-positive guard. The remaining pre-execution decisions are settled: staging dir is `resources/css/` with a root `.gitignore` entry, and the release workflow injects the generated file into the tag-archived staging tree rather than changing what gets archived.
