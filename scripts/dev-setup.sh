#!/usr/bin/env bash
set -euo pipefail

# Install the local development dependencies that the build and test suite
# need but that are not tracked in git.
#
#   - scripts/compiler-runtime/node_modules — the Svelte compiler bridge
#     src/compiler.rs drives the compiler through a JavaScript runtime, and the
#     compiler fixture tests under `cargo test --lib` fail without these
#     packages.
#   - scripts/get-fixtures/node_modules — the upstream parity harness
#     (scripts/get-fixtures/import.mjs, npm test there).
#   - fixtures/parser/upstream/ and fixtures/linter/upstream/ (+
#     manifest.json) — the pinned upstream fixture trees, fetched by the
#     `cargo get-svelte-fixtures` and `cargo get-eslint-fixtures` aliases when
#     missing or empty (see docs/research/test/pinned-rules.md).
#
# JavaScript dependencies are installed with bun when it is on PATH (bun
# replaces both node and npm), otherwise with npm via node. Safe to run
# repeatedly: a directory is only reinstalled when its node_modules is missing
# or its package.json is newer than the last install, and the fixture trees
# are only fetched when empty. release.sh runs this script before building
# and testing.
#
# Usage: dev-setup.sh [--skip-cargo-tests]
#   --skip-cargo-tests  Do not run `cargo test --release` after fetching
#                       fixtures (by default the full release-profile test
#                       suite runs whenever fixtures were fetched).

cd "$(dirname "$0")/.."

SCRIPTS_BIN=()
SKIP_CARGO_TESTS=false

for arg in "$@"; do
    case "$arg" in
        --skip-cargo-tests) SKIP_CARGO_TESTS=true ;;
        *)
            printf '%s\n' "error: unknown argument '$arg' (supported: --skip-cargo-tests)" >&2
            exit 2
            ;;
    esac
done

# 1. Populate paths
for tool in bun node npm cargo; do
    if path=$(which "$tool" 2>/dev/null) && [ -n "$path" ]; then
        SCRIPTS_BIN+=("$path")
    fi
done

if [[ "${SCRIPTS_BIN[*]}" != *"/bun"* && "${SCRIPTS_BIN[*]}" != *"/node"* ]]; then
    printf '%s%s\n' \
        "Error: Neither 'bun' nor 'node' JavaScript runtimes were found in PATH. " \
        "At least one runtime is required to continue running '$0'!" >&2
    exit 1
elif [[ "${SCRIPTS_BIN[*]}" != *"/cargo"* ]]; then
    printf '%s\n' "error: 'cargo' is required but was not found on PATH." >&2
    exit 1
fi

# 2. Designate the JavaScript runtime: bun preferred, node fallback. node
# cannot install packages by itself, so on the node-only path js_runtime
# holds npm's path and the install runs through it.
js_runtime=""
for bin in "${SCRIPTS_BIN[@]}"; do
    case "$bin" in
        */bun) js_runtime="$bin"; break ;;
    esac
done
if [ -z "$js_runtime" ]; then
    if [[ "${SCRIPTS_BIN[*]}" != *"/npm"* ]]; then
        printf '%s%s%s\n' "error: " \
            "'node' was found but 'npm' was not; " \
            "install npm, or install bun to replace both." >&2
        exit 1
    fi
    for bin in "${SCRIPTS_BIN[@]}"; do
        case "$bin" in
            */npm) js_runtime="$bin"; break ;;
        esac
    done
fi

js_projects=(
  scripts/compiler-runtime
  scripts/get-fixtures
)

for dir in "${js_projects[@]}"; do
    stamp="${dir}/node_modules/.dev-setup-stamp"
    if [ ! -d "${dir}/node_modules" ]; then
        echo "Installing JS dependencies in ${dir} with $(basename "$js_runtime") (node_modules is missing)..."
    elif [ ! -f "$stamp" ] || [ "${dir}/package.json" -nt "$stamp" ]; then
        echo "Reinstalling JS dependencies in ${dir} with $(basename "$js_runtime") (package.json changed)..."
    else
        echo "${dir}: JS dependencies up to date."
        continue
    fi
    (cd "$dir" && "$js_runtime" install --ignore-scripts)
    touch "$stamp"
done

# Fetch the pinned upstream fixture trees when they are missing or empty.
# import.mjs creates these directories on first sync; our hand-maintained
# cases live in the sibling local/ roots and never factor in.
needs_fixtures() {
    [ -d "$1" ] || return 0
    [ -z "$(find "$1" -mindepth 1 -print -quit)" ]
}

changed_fixtures=""

if needs_fixtures fixtures/parser/upstream; then
    echo "Fetching parser fixtures (cargo get-svelte-fixtures)..."
    cargo get-svelte-fixtures
    changed_fixtures="1"
else
    echo "fixtures/parser: parser fixtures already populated."
fi

if needs_fixtures fixtures/linter/upstream; then
    echo "Fetching ESLint rule fixtures (cargo get-eslint-fixtures)..."
    cargo get-eslint-fixtures
    changed_fixtures="1"
else
    echo "fixtures/linter/upstream: ESLint rule fixtures already populated."
fi

# Freshly fetched fixtures have never been exercised on this machine — prove
# them (and the freshly built tree) with the full release-profile test suite,
# unless --skip-cargo-tests was passed.
if [ "$changed_fixtures" = "1" ] && [ "$SKIP_CARGO_TESTS" != "true" ]; then
    echo "Fixtures were fetched; running the full test suite (cargo test --release)..."
    cargo test --release
fi

echo "Dev dependencies are ready."
