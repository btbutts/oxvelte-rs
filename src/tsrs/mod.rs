//! Client for driving an external `tsrs --api` sidecar over the tuple
//! framing in `crate::transport::message_pack`. Spawning, session
//! lifecycle, and the typed request helpers the conditions port uses.
//! See `docs/research/dependency/tsrs-type-aware-checking.md`.

pub mod protocol;
pub mod resolve;
pub mod session;

use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::transport::TransportError;
use crate::transport::message_pack::{MessagePackReader, MessagePackWriter, MessageType};

#[derive(Debug, thiserror::Error)]
pub enum TsrsError {
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("failed to spawn tsrs: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("tsrs server error for `{method}`: {message}")]
    Server { method: String, message: String },
    #[error("malformed response for `{method}`: {source}")]
    Json { method: String, source: serde_json::Error },
    #[error("unexpected message type {found:?} for `{method}`")]
    UnexpectedMessage { method: String, found: MessageType },
}

/// A live `tsrs --api` sidecar. Requests are strictly sequential: the
/// protocol has no request IDs and correlates responses by method name.
/// The child is killed on drop.
pub struct TsrsClient {
    child: Child,
    writer: MessagePackWriter<ChildStdin>,
    reader: MessagePackReader<ChildStdout>,
}

impl TsrsClient {
    /// Spawns `tsrs --api` rooted at `cwd` and performs the `initialize`
    /// handshake.
    pub fn spawn(binary: &Path, cwd: &Path) -> Result<Self, TsrsError> {
        let mut child = Command::new(binary)
            .arg("--api")
            .arg("-cwd")
            .arg(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let mut client = Self {
            child,
            writer: MessagePackWriter::new(stdin),
            reader: MessagePackReader::new(stdout),
        };
        client.initialize()?;
        Ok(client)
    }

    /// Sends one request and decodes the typed response.
    pub fn request<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
    ) -> Result<R, TsrsError> {
        let payload = serde_json::to_vec(params).map_err(|source| TsrsError::Json {
            method: method.to_owned(),
            source,
        })?;
        self.writer
            .write_tuple(MessageType::Request, method.as_bytes(), &payload)?;
        let tuple = self.reader.read_tuple()?;
        match tuple.msg_type {
            MessageType::Response => serde_json::from_slice(&tuple.payload).map_err(|source| TsrsError::Json {
                method: method.to_owned(),
                source,
            }),
            MessageType::Error => Err(TsrsError::Server {
                method: method.to_owned(),
                message: String::from_utf8_lossy(&tuple.payload).into_owned(),
            }),
            found => Err(TsrsError::UnexpectedMessage {
                method: method.to_owned(),
                found,
            }),
        }
    }

    fn initialize(&mut self) -> Result<protocol::InitializeResponse, TsrsError> {
        self.request("initialize", &serde_json::Value::Null)
    }

    /// Opens a configured project by its tsconfig and returns the new
    /// snapshot id plus the project handle.
    pub fn create_snapshot(&mut self, tsconfig: &Path) -> Result<(u64, String), TsrsError> {
        let resp: protocol::CreateSnapshotResponse = self.request(
            "createSnapshot",
            &serde_json::json!({ "openProjects": [tsconfig.to_string_lossy()] }),
        )?;
        let project = resp.projects.first().map(|p| p.id.clone()).unwrap_or_default();
        Ok((resp.snapshot, project))
    }

    /// The type of the expression at a UTF-16 position in a file.
    pub fn get_type_at_position(
        &mut self,
        snapshot: u64,
        project: &str,
        file: &Path,
        position: u32,
    ) -> Result<protocol::TypeResponse, TsrsError> {
        self.request(
            "getTypeAtPosition",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "file": file.to_string_lossy(),
                "position": position,
            }),
        )
    }

    /// The union/intersection parts of a type (never inline on the type
    /// itself). Note the parameter is `objectId`, not `type`.
    pub fn get_types_of_type(
        &mut self,
        snapshot: u64,
        project: &str,
        type_id: u32,
    ) -> Result<Vec<protocol::TypeResponse>, TsrsError> {
        self.request(
            "getTypesOfType",
            &serde_json::json!({ "snapshot": snapshot, "project": project, "objectId": type_id }),
        )
    }

    /// Semantic diagnostics for one file.
    pub fn get_semantic_diagnostics(
        &mut self,
        snapshot: u64,
        project: &str,
        file: &Path,
    ) -> Result<Vec<protocol::DiagnosticResponse>, TsrsError> {
        self.request(
            "getSemanticDiagnostics",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "files": [file.to_string_lossy()],
            }),
        )
    }
}

impl Drop for TsrsClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const SOURCE: &str = "const definitely: number = 42;\nconst bad: number = \"oops\";\n";

    /// Skips when no tsrs binary is on PATH.
    fn scratch_project() -> Option<(TsrsClient, PathBuf)> {
        let binary = resolve::find_on_path()?;
        let dir = std::env::temp_dir().join("oxvelte-tsrs-client-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("tsconfig.json"),
            r#"{"compilerOptions":{"strict":true,"noEmit":true}}"#,
        )
        .unwrap();
        fs::write(dir.join("a.ts"), SOURCE).unwrap();
        let client = TsrsClient::spawn(&binary, &dir).expect("spawn + initialize");
        Some((client, dir))
    }

    #[test]
    fn client_session_round_trip() {
        let Some((mut client, dir)) = scratch_project() else {
            eprintln!("skipping: no tsrs binary on PATH");
            return;
        };
        let tsconfig = dir.join("tsconfig.json");
        let file = dir.join("a.ts");

        let (snapshot, project) = client.create_snapshot(&tsconfig).expect("createSnapshot");
        assert!(!project.is_empty());

        let position = SOURCE.find("definitely").unwrap() as u32;
        let ty = client
            .get_type_at_position(snapshot, &project, &file, position)
            .expect("getTypeAtPosition");
        assert_eq!(ty.intrinsic_name.as_deref(), Some("number"));

        let diagnostics = client
            .get_semantic_diagnostics(snapshot, &project, &file)
            .expect("diagnostics");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, 2322);

        let _ = fs::remove_dir_all(&dir);
    }
}
