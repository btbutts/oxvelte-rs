//! Thin launcher behind the `cargo get-eslint-fixtures` and
//! `cargo get-svelte-fixtures` aliases (see .cargo/config.toml). It forwards
//! the pinned release tag to scripts/get-fixtures/import.mjs and runs it with
//! bun when available, node otherwise. All import logic lives in the script.

use std::env;
use std::path::Path;
use std::process::{exit, Command, Stdio};

const SCRIPT_DIR: &str = "scripts/get-fixtures";
const SCRIPT: &str = "import.mjs";

#[derive(Clone, Copy)]
enum Runtime {
    Bun,
    Node,
}

impl Runtime {
    fn program(self) -> &'static str {
        match self {
            Runtime::Bun => "bun",
            Runtime::Node => "node",
        }
    }
}

fn on_path(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn select_runtime() -> Runtime {
    if let Ok(forced) = env::var("OXVELTE_JS_RUNTIME") {
        match forced.as_str() {
            "bun" | "node" if on_path(&forced) => {
                return if forced == "bun" { Runtime::Bun } else { Runtime::Node };
            }
            "bun" | "node" => {
                eprintln!("error: OXVELTE_JS_RUNTIME={forced} but `{forced}` was not found on PATH");
                exit(2);
            }
            other => {
                eprintln!("error: OXVELTE_JS_RUNTIME must be \"bun\" or \"node\" (got \"{other}\")");
                exit(2);
            }
        }
    }
    if on_path("bun") {
        Runtime::Bun
    } else if on_path("node") {
        Runtime::Node
    } else {
        eprintln!("error: neither `bun` nor `node` was found on PATH");
        exit(2);
    }
}

fn ensure_dependencies(script_dir: &Path, runtime: Runtime) {
    if script_dir.join("node_modules").is_dir() {
        return;
    }
    // npm is a shell script on Windows, so it must be launched via its .cmd shim.
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let (program, args): (&str, &[&str]) = match runtime {
        Runtime::Bun => ("bun", &["install"]),
        Runtime::Node => (npm, &["ci", "--ignore-scripts", "--no-audit", "--no-fund"]),
    };
    println!("Installing {SCRIPT_DIR} dependencies ({program} {})...", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(script_dir)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run `{program}` in {SCRIPT_DIR}: {e}");
            exit(1);
        });
    if !status.success() {
        eprintln!("error: dependency install failed in {SCRIPT_DIR}");
        exit(1);
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let job = args.first().map(String::as_str).unwrap_or("");
    let pin_var = match job {
        "--get-svelte-fixtures" => "OXVELTE_FIXTURES_SVELTE_TAG",
        "--get-eslint-fixtures" => "OXVELTE_FIXTURES_ESLINT_PLUGIN_SVELTE_TAG",
        _ => {
            eprintln!("usage: launcher --get-svelte-fixtures | --get-eslint-fixtures");
            eprintln!("normally run via the cargo aliases `cargo get-svelte-fixtures` / `cargo get-eslint-fixtures`");
            exit(2);
        }
    };
    let tag = env::var(pin_var).unwrap_or_else(|_| {
        eprintln!("error: {pin_var} is not set; run through the cargo alias so .cargo/config.toml [env] applies");
        exit(2);
    });

    let runtime = select_runtime();
    let script_dir = Path::new(SCRIPT_DIR);
    ensure_dependencies(script_dir, runtime);

    let mut command = Command::new(runtime.program());
    match runtime {
        // `--bun` forces Bun's own runtime; without it bun may defer to node.
        Runtime::Bun => command.args(["run", "--bun", SCRIPT]),
        Runtime::Node => command.arg(SCRIPT),
    };
    let status = command
        .arg(job)
        .current_dir(script_dir)
        .env(pin_var, &tag)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to launch {}: {e}", runtime.program());
            exit(1);
        });
    exit(status.code().unwrap_or(1));
}
