<p align="center">
  <img src="assets/oxvelte.svg" alt="oxvelte logo" width="200">
</p>

# oxvelte

> [!NOTE]
> **This is a fork of [tolgaouz/oxvelte](https://github.com/tolgaouz/oxvelte).** oxvelte was created by [@tolgaouz](https://github.com/tolgaouz), and all credit for the original design and implementation goes to him. Further credit is due to [billpeet/oxvelte](https://github.com/billpeet/oxvelte) who worked to add additional fixtures, upstream parity testing, among others. The original repository looks to be no longer actively developed, so this fork ([btbutts/oxvelte-rs](https://github.com/btbutts/oxvelte-rs)) continues the work. It is not affiliated with or endorsed by either @billpeet or @tolgaouz.

A Svelte linter with native Rust rules and optional bun/Node.js compiler and type checks. All 1,316 eligible fixtures in the pinned [eslint-plugin-svelte](https://github.com/sveltejs/eslint-plugin-svelte) suite match. See the [parity report](docs/research/upstream-parity-seventh-wave.md) for scope and runtime requirements.

All 1,362 imported cases pass across the [pinned version matrix](docs/research/upstream-parity-version-matrix.md), including the 67 cases skipped in the primary environment.

<p align="center">
  <img src="assets/compare.gif" alt="Side-by-side benchmark linting shadcn-svelte (1,603 files): eslint-plugin-svelte takes ~15s while oxvelte completes the same lint hundreds of times in the same window" width="900">
</p>

> **This codebase was written mainly with LLM assistance.**

## Using with oxlint (recommended setup)

The fastest way to lint a SvelteKit project is **oxlint + oxvelte** together. They handle different concerns with zero overlap:

| Tool | What it lints | File types |
|------|--------------|------------|
| [oxlint](https://oxc.rs/docs/guide/usage/linter) | General JS/TS rules (`no-unused-vars`, `no-console`, type checks, imports, etc.) | `.js`, `.ts`, `.svelte` (`<script>` blocks only) |
| **oxvelte** | Svelte-specific rules (`infinite-reactive-loop`, `no-at-html-tags`, template issues, etc.) | `.svelte` |

oxlint can lint the JavaScript/TypeScript inside `.svelte` `<script>` blocks, but it does not lint Svelte templates, styles, or Svelte-specific component semantics. oxvelte covers that Svelte-specific layer: reactive patterns, template structure, style-related checks, and component conventions.

### Quick setup

#### - NPM packages are comming soon
#### - MacOS and Linux Users:
```bash
# Install with homebrew
brew tap btbutts/homebrew-oxvelte
brew install oxvelte oxlint
```

That's it. Both tools work out of the box with zero config and sensible defaults. There is no requirement to install oxlint, but it is certainly reccomended! 😎

Compiler diagnostics and unused `svelte-ignore` checks use a JavaScript runtime — **bun when available, otherwise Node.js** — and the project's installed Svelte compiler. `svelte/valid-compile` is opt-in; the recommended unused-ignore rule starts the compiler when it needs to check warning codes. TypeScript, Babel and stylesheet transformations use the project's corresponding packages. The compiler process is reused, and compiler results are shared between rules within each lint run. JS runtime selection can be pinned via `settings["js-runtime"]` in your `oxvelte.config.json` (see Configuration) or the `OXVELTE_NODE` environment variable. See [compiler setup and parity testing](docs/upstream-parity.md) for the pinned test runtime.

The opt-in `@typescript-eslint/no-unnecessary-condition` rule uses the project's TypeScript checker to report redundant conditions and optional chains while respecting Svelte reactive variables. It runs when the config sets it to `warn` or `error`, or with `--all-rules`; `settings.typescript.project` can select one tsconfig path relative to the component. On Svelte 5 projects running TypeScript 6 or newer, the checker can optionally be [tsrs](https://github.com/oxc-project/tsrs) — the Rust port of the TypeScript 7 checker — instead of the JavaScript sidecar; see `settings.tsrs` under Configuration. See the [seventh-wave report](docs/research/upstream-parity-seventh-wave.md) for template scope limits and current test coverage.

The opt-in `@typescript-eslint/no-unused-vars` rule reports unused imports, variables, functions, parameters and caught errors in a component's `<script>` blocks. Script-only linters such as oxlint skip this check in `.svelte` files, because they cannot see whether the template uses a name. This rule counts template usage, `$store` subscriptions, `$bindable()` props and names shared between the module and instance scripts. Switch it on in `oxvelte.config.json`, with the same options typescript-eslint takes:

```json
{
  "rules": {
    "@typescript-eslint/no-unused-vars": [
      "error",
      { "argsIgnorePattern": "^_", "varsIgnorePattern": "^_", "caughtErrorsIgnorePattern": "^_", "destructuredArrayIgnorePattern": "^_" }
    ]
  }
}
```

It reports at the same position and with the same message as typescript-eslint. It stays quiet when it cannot prove a name is unused: template usage is matched by name without regard to shadowing, and a function that only calls itself counts as used. It does not yet check names declared in the template (`{#each}` indexes, snippet parameters, parameters of inline arrow functions) or type parameters in the `generics` attribute. On two production apps it matched 593 of typescript-eslint's 624 findings in `.svelte` files exactly and reported nothing else.

A rule outside the recommended set runs when the config sets it to `warn` or `error`. `--all-rules` is no longer needed to opt in to one rule.

### Replacing eslint-plugin-svelte + ESLint

If you're migrating from the ESLint setup (`eslint-plugin-svelte` + `@eslint/js` + `typescript-eslint`), here's how the tools map:

| ESLint stack | Replacement |
|-------------|-------------|
| `@eslint/js` (core JS rules) | `oxlint` |
| `typescript-eslint` | `oxlint --tsconfig` |
| `eslint-plugin-svelte` | **`oxvelte`** |
| `eslint-plugin-import` | `oxlint` (built-in `--import-plugin`) |

```bash
# Before (ESLint, ~3-10 seconds)
eslint src/

# After (oxlint + oxvelte, ~200-400ms)
oxlint && oxvelte lint src/
```

If you have an existing eslint-plugin-svelte config, oxvelte can convert it:

```bash
oxvelte migrate eslint.config.js --write
```

### Full SvelteKit example

For a typical SvelteKit project with TypeScript:

```json
{
  "scripts": {
    "lint": "oxlint --tsconfig tsconfig.json && oxvelte lint src/",
    "lint:fix": "oxlint --fix --tsconfig tsconfig.json && oxvelte lint --fix src/"
  },
  "devDependencies": {
    "oxlint": "^1.58.0"
  }
}
```

```bash
# optional: configure oxlint
# .oxlintrc.json
{
  "plugins": ["typescript", "import", "unicorn"],
  "rules": {
    "no-console": "warn"
  }
}

# optional: configure oxvelte
# oxvelte.config.json
{
  "rules": {
    "svelte/no-at-html-tags": "error"
  },
  "settings": {
    "svelte": {
      "kit": {
        "files": { "routes": "src/routes" }
      }
    }
  }
}
```

### CI integration

Both tools return non-zero exit codes on errors, so they work naturally in CI:

```yaml
# GitHub Actions
- run: npx oxlint --tsconfig tsconfig.json
- run: npx oxvelte lint src/
```

For JSON output (useful for custom reporters or IDE integration):

```bash
oxlint --format json
oxvelte lint --json src/
```

## Install

### From GitHub

```bash
cargo install --git https://github.com/btbutts/oxvelte-rs.git
```

### From source

```bash
git clone https://github.com/btbutts/oxvelte-rs.git
cd oxvelte
cargo build --release
```

The binary will be at `./target/release/oxvelte`.

## Usage

```bash
oxvelte lint src/              # lint with recommended rules
oxvelte lint --json src/       # JSON output
oxvelte lint --fix src/        # auto-fix where supported
oxvelte lint --all-rules src/  # run all 78 rules
oxvelte rules                  # list available rules
```

## Configuration

Create `oxvelte.config.json` in your project root:

```json
{
  "rules": {
    "svelte/no-at-html-tags": "error",
    "svelte/button-has-type": ["warn", { "button": false }],
    "svelte/no-inline-styles": "off"
  },
  "settings": {
    "svelte": {
      "kit": {
        "files": { "routes": "src/routes" }
      }
    }
  }
}
```

**Rules** use the same names and options as eslint-plugin-svelte. Severity can be `"off"`, `"warn"`, or `"error"` (or `0`, `1`, `2`). Options are passed as the second element of a tuple: `["error", { ...options }]`.

**Settings** configure framework-specific behavior. The `svelte.kit.files.routes` setting tells SvelteKit-aware rules where your route files live.

**JavaScript runtime.** Compiler-backed and type-aware rules launch a JavaScript runtime for the embedded compiler bridge. By default oxvelte prefers `bun` and falls back to `node` from your PATH. Pin one with `settings["js-runtime"]`:

```json
{
  "settings": {
    "js-runtime": "node"
  }
}
```

...which forces that runtime (resolved from PATH), or pin an exact binary with the name-plus-path form (the name tells oxvelte which runtime the path is):

```json
{
  "settings": {
    "js-runtime": ["bun", { "path": "/opt/homebrew/bin/bun" }]
  }
}
```

The `OXVELTE_NODE` environment variable overrides both when set (a name or absolute path). If neither `bun` nor `node` is found, oxvelte tells you to install one or set an override.

**Type-aware engine (optional).** The `@typescript-eslint/no-unnecessary-condition` rule needs a TypeScript checker and uses the JavaScript runtime above with your project's own `typescript` package. On Svelte 5 projects running TypeScript 6 or newer, you can route type checking through [tsrs](https://github.com/oxc-project/tsrs) — the Rust port of the TypeScript 7 checker — with no JavaScript sidecar involved:

```json
{
  "settings": {
    "tsrs": true
  }
}
```

`settings.tsrs` defaults to `false`; setting it changes nothing else about how oxvelte runs. tsrs is a separate binary that oxvelte never downloads or compiles: it resolves `node_modules/.bin/tsrs` walking up from the linted file first, then `tsrs` from your PATH, so install it however you prefer (the Homebrew tap installs it automatically as a dependency of the `oxvelte` formula). Release binaries are built with tsrs support compiled in; source builds need `--features tsrs-inference`, and without it the setting is ignored. When `settings.tsrs` is on but the component is not Svelte 5, the project's TypeScript is older than 6, or no binary resolves, the rule keeps using the JavaScript sidecar — set `OXVELTE_TSRS_TRACE=1` to print which engine served each file and why.

Without a config file, oxvelte runs the **recommended** ruleset (same as eslint-plugin-svelte's `flat/recommended`).

## Custom rules

Project-specific conventions that don't belong in the shared ruleset — forbidden imports, naming schemes, required attributes — can be written in JavaScript and loaded via `customRules`:

```json
{
  "rules": { "custom/no-div-without-class": "error" },
  "customRules": ["./rules/*.js"]
}
```

```javascript
// ./rules/no-div-without-class.js
export default {
  name: "custom/no-div-without-class",
  run(ctx) {
    ctx.walk((node) => {
      if (node.type === "Element" && node.name === "div") {
        const hasClass = node.attributes.some(
          (a) => a.type === "NormalAttribute" && a.name === "class",
        );
        if (!hasClass) ctx.diagnostic("div must have a class attribute", node.span);
      }
    });
  },
};
```

Rules run in an embedded [Boa](https://boajs.dev/) engine — no Node.js dependency, no IPC. The npm package includes this; when building from source it requires the `custom-rules` feature (`cargo install … --features custom-rules`).

Full reference — AST shape, `ctx` API, auto-fix, limitations — in [`docs/custom-rules.md`](docs/custom-rules.md).

## What's implemented

Upstream compatibility is tracked separately with a pinned fixture suite and exact expectations. See [upstream parity](docs/upstream-parity.md) for current gaps and how to run it.

- **Svelte lint rules** implemented in Rust, with bun/Node.js for compiler and type checks
- **Full Svelte 4 + Svelte 5** template parser (106/106 parser fixture tests)
- **1,293 upstream rule fixtures match**, with 67 version skips
- **Parallel file processing** via rayon
- **eslint-disable** / **svelte-ignore** comment directives
- **Auto-fix** support for fixable rules (`--fix`)

### Compiler and formatting rules

Compiler-backed and formatting rules are supported:

- `valid-compile` invokes the project's Svelte compiler and remains opt-in.
- `no-unused-svelte-ignore` is recommended and invokes the compiler when it needs to check warning codes.
- `indent` is opt-in, matching upstream. Avoid enabling it alongside a formatter that uses different indentation settings.

Compiler checks require a JavaScript runtime (bun or Node.js) and the relevant project packages. The [parity report](docs/research/upstream-parity-seventh-wave.md) records fixture coverage and implementation limits.

## Project structure

```
src/                    all Rust code
  main.rs               CLI entry point
  parser/               Svelte template parser
  linter/rules/         lint rules (one file per rule)
  ast.rs                Svelte AST types
Cargo.toml              dependencies
README.md               you are here
```

## License

[MIT](LICENSE). Contributor copyright © [@btbutts](https://github.com/btbutts)<br>
[MIT](LICENSE). Contributor copyright © [@billpeet](https://github.com/billpeet) <br>
[MIT](LICENSE). Original work copyright © [@tolgaouz](https://github.com/tolgaouz)