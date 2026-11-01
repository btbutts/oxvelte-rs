//! Which typing engine served a lint run, shared by the dual-backend test
//! suites. One source of truth for the printed label, availability probing,
//! and forcing the engine in a case's `settings` JSON — so a failure always
//! names its engine and no test logic is duplicated per backend.

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypingEngine {
    /// The Node/Bun sidecar running the project's own `typescript` package.
    JsSidecar,
    /// The external `tsrs --api` binary (TypeScript 7 checker).
    Tsrs,
}

impl TypingEngine {
    /// Printed in test names, failure messages and report headers.
    pub fn label(self) -> &'static str {
        match self {
            TypingEngine::JsSidecar => "js-sidecar",
            TypingEngine::Tsrs => "tsrs",
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "js-sidecar" | "js" => Ok(TypingEngine::JsSidecar),
            "tsrs" => Ok(TypingEngine::Tsrs),
            other => Err(format!("Unknown engine: {other} (expected js-sidecar or tsrs)")),
        }
    }

    /// Whether this engine can run here: tsrs needs the feature compiled in
    /// and a binary on PATH; the JS sidecar is always available.
    pub fn available(self) -> bool {
        match self {
            TypingEngine::JsSidecar => true,
            TypingEngine::Tsrs => {
                cfg!(feature = "tsrs-inference") && std::process::Command::new("tsrs").arg("--version").output().is_ok()
            }
        }
    }

    /// Forces this engine in a case's `settings` JSON (`settings.tsrs`).
    /// The rule's routing gate still applies: a tsrs-forced run on a
    /// Svelte < 5 or TypeScript < 6 project falls back to the JS sidecar,
    /// which is exactly what the version-matrix fallback legs assert.
    pub fn apply(self, settings: &mut Value) {
        settings["tsrs"] = Value::Bool(matches!(self, TypingEngine::Tsrs));
    }
}

impl std::fmt::Display for TypingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
