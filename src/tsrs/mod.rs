//! Client for driving an external `tsrs --api` sidecar over the tuple
//! framing in `crate::transport::message_pack`. Spawning, session
//! lifecycle, and the typed request helpers the conditions port uses.
//! See `docs/research/dependency/tsrs-type-aware-checking.md`.

pub mod binary_ast;
pub mod conditions;
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
    /// From `initialize`: 0 means case-insensitive paths (lowercase path
    /// keys), non-zero means case-sensitive.
    pub case_sensitivity: i32,
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
            case_sensitivity: 0,
        };
        client.case_sensitivity = client.initialize()?.case_sensitivity;
        Ok(client)
    }

    /// Sends one request with a raw payload and returns the raw response
    /// payload. Some responses are raw binary rather than JSON (e.g.
    /// `getSourceFile` when the server uses binary responses).
    pub fn request_raw(&mut self, method: &str, payload: &[u8]) -> Result<Vec<u8>, TsrsError> {
        self.writer
            .write_tuple(MessageType::Request, method.as_bytes(), payload)?;
        let tuple = self.reader.read_tuple()?;
        match tuple.msg_type {
            MessageType::Response => Ok(tuple.payload),
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
        let raw = self.request_raw(method, &payload)?;
        serde_json::from_slice(&raw).map_err(|source| TsrsError::Json {
            method: method.to_owned(),
            source,
        })
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

    /// The binary-encoded AST of a file, for constructing node handles — see
    /// `binary_ast`. The sync server answers with raw binary (not the
    /// JSON/base64 wrapper) when binary responses are enabled.
    pub fn get_source_file_data(&mut self, snapshot: u64, project: &str, file: &Path) -> Result<Vec<u8>, TsrsError> {
        let params = serde_json::to_vec(&serde_json::json!({
            "snapshot": snapshot,
            "project": project,
            "file": file.to_string_lossy(),
        }))
        .map_err(|source| TsrsError::Json {
            method: "getSourceFile".to_owned(),
            source,
        })?;
        let raw = self.request_raw("getSourceFile", &params)?;
        if raw.first() == Some(&b'{') {
            let resp: serde_json::Value = serde_json::from_slice(&raw).map_err(|source| TsrsError::Json {
                method: "getSourceFile".to_owned(),
                source,
            })?;
            let encoded = resp["data"].as_str().unwrap_or_default();
            return crate::tsrs::binary_ast::decode_base64(encoded).ok_or_else(|| TsrsError::Server {
                method: "getSourceFile".to_owned(),
                message: "invalid base64 AST data".to_owned(),
            });
        }
        Ok(raw)
    }

    /// The type of an exact node by its `index.kind.pathKey` handle.
    /// Nullable: the server answers `null` when no type applies.
    pub fn get_type_at_location(
        &mut self,
        snapshot: u64,
        project: &str,
        location: &str,
    ) -> Result<Option<protocol::TypeResponse>, TsrsError> {
        self.request(
            "getTypeAtLocation",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "location": location,
            }),
        )
    }

    /// The base constraint of a type, or `None` when it has none (the
    /// sidecar's `type()` then uses the type itself). Note the parameter is
    /// `type` here (CheckerTypeParams), not `objectId`.
    pub fn get_base_constraint_of_type(
        &mut self,
        snapshot: u64,
        project: &str,
        type_id: u32,
    ) -> Result<Option<protocol::TypeResponse>, TsrsError> {
        self.request(
            "getBaseConstraintOfType",
            &serde_json::json!({ "snapshot": snapshot, "project": project, "type": type_id }),
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

    /// Syntactic diagnostics for one file.
    pub fn get_syntactic_diagnostics(
        &mut self,
        snapshot: u64,
        project: &str,
        file: &Path,
    ) -> Result<Vec<protocol::DiagnosticResponse>, TsrsError> {
        self.request(
            "getSyntacticDiagnostics",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "files": [file.to_string_lossy()],
            }),
        )
    }

    /// The symbol at a node handle. Nullable.
    pub fn get_symbol_at_location(
        &mut self,
        snapshot: u64,
        project: &str,
        location: &str,
    ) -> Result<Option<protocol::SymbolResponse>, TsrsError> {
        self.request(
            "getSymbolAtLocation",
            &serde_json::json!({ "snapshot": snapshot, "project": project, "location": location }),
        )
    }

    /// The value symbol of a shorthand property assignment. Nullable.
    pub fn get_shorthand_assignment_value_symbol(
        &mut self,
        snapshot: u64,
        project: &str,
        location: &str,
    ) -> Result<Option<protocol::SymbolResponse>, TsrsError> {
        self.request(
            "getShorthandAssignmentValueSymbol",
            &serde_json::json!({ "snapshot": snapshot, "project": project, "location": location }),
        )
    }

    /// Whether the type is an array type (CheckerTypeParams: `type`).
    pub fn is_array_type(&mut self, snapshot: u64, project: &str, type_id: u32) -> Result<bool, TsrsError> {
        self.request(
            "isArrayType",
            &serde_json::json!({ "snapshot": snapshot, "project": project, "type": type_id }),
        )
    }

    /// A named property of a type. Nullable.
    pub fn get_property_of_type(
        &mut self,
        snapshot: u64,
        project: &str,
        type_id: u32,
        name: &str,
    ) -> Result<Option<protocol::SymbolResponse>, TsrsError> {
        self.request(
            "getPropertyOfType",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "type": type_id,
                "name": name,
            }),
        )
    }

    /// The type of a symbol at a location. Nullable.
    pub fn get_type_of_symbol_at_location(
        &mut self,
        snapshot: u64,
        project: &str,
        symbol: &protocol::SymbolReference,
        location: &str,
    ) -> Result<Option<protocol::TypeResponse>, TsrsError> {
        self.request(
            "getTypeOfSymbolAtLocation",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "symbol": symbol,
                "location": location,
            }),
        )
    }

    /// Index info presence for a type (`kind`: 0 string, 1 number). Nullable.
    pub fn get_index_info_of_type(
        &mut self,
        snapshot: u64,
        project: &str,
        type_id: u32,
        kind: i32,
    ) -> Result<Option<serde_json::Value>, TsrsError> {
        self.request(
            "getIndexInfoOfType",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "type": type_id,
                "kind": kind,
            }),
        )
    }

    /// Call/construct signatures of a type (`kind`: 0 call, 1 construct).
    pub fn get_signatures_of_type(
        &mut self,
        snapshot: u64,
        project: &str,
        type_id: u32,
        kind: i32,
    ) -> Result<Vec<protocol::SignatureResponse>, TsrsError> {
        self.request(
            "getSignaturesOfType",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "type": type_id,
                "kind": kind,
            }),
        )
    }

    /// The return type of a signature (GetSignaturePropertyParams:
    /// `objectId`). Nullable.
    pub fn get_return_type_of_signature(
        &mut self,
        snapshot: u64,
        project: &str,
        signature_id: u64,
    ) -> Result<Option<protocol::TypeResponse>, TsrsError> {
        self.request(
            "getReturnTypeOfSignature",
            &serde_json::json!({
                "snapshot": snapshot,
                "project": project,
                "objectId": signature_id,
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
