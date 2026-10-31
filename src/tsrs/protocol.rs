//! Serde types for the `tsrs --api` methods we call. Field names and shapes
//! follow microsoft/TypeScript `tsc/internal/api/proto.go` (the protocol
//! tsrs implements); payloads are JSON inside the tuple framing.
//!
//! Handles on the wire: snapshots are u64, projects are strings (the
//! tsconfig path), types are u32 ids, symbols/signatures u64, nodes string
//! handles, and positions are UTF-16 offsets.

use serde::Deserialize;

/// `initialize` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub case_sensitivity: i32,
    pub current_directory: String,
}

/// A project entry in a `createSnapshot` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResponse {
    /// The project handle: the tsconfig path for configured projects.
    pub id: String,
    #[serde(default)]
    pub config_file_name: Option<String>,
    #[serde(default)]
    pub current_directory: String,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub parsed_command_line: Option<ParsedCommandLineResponse>,
}

/// The parsed tsconfig of a configured project, as reported in a
/// `createSnapshot` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCommandLineResponse {
    #[serde(default)]
    pub file_names: Vec<String>,
    #[serde(default)]
    pub options: serde_json::Value,
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
}

/// `createSnapshot` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSnapshotResponse {
    pub snapshot: u64,
    #[serde(default)]
    pub projects: Vec<ProjectResponse>,
}

/// Identifies a cached symbol without its owning file's full descriptor.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactSymbolReference {
    pub id: u64,
    #[serde(default)]
    pub file: Option<String>,
}

/// `TypeResponse`: a type handle with its inline data. `flags` are TS7's
/// `TypeFlags` bits — renumbered from TS <= 5 (null 0x8, string 0x20,
/// number 0x40, string literal 0x400, union 0x8000000); the conditions port
/// must use the TS7 values. Union/intersection parts are never inline —
/// fetch them with `getTypesOfType`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeResponse {
    pub id: u32,
    pub flags: u32,
    #[serde(default)]
    pub object_flags: u32,
    #[serde(default)]
    pub is_tuple_type: bool,
    /// Literal type data (string/number/bool literal values); absent is null.
    #[serde(default)]
    pub value: serde_json::Value,
    /// ObjectType / TypeReference / IndexType / StringMappingType target.
    #[serde(default)]
    pub target: Option<u32>,
    /// FreshableType (literal / computed enum) handles.
    #[serde(default)]
    pub fresh_type: Option<u32>,
    #[serde(default)]
    pub regular_type: Option<u32>,
    #[serde(default)]
    pub intrinsic_name: Option<String>,
    /// Symbol associated with structured types.
    #[serde(default)]
    pub symbol: Option<CompactSymbolReference>,
    #[serde(default)]
    pub type_parameters: Vec<u32>,
    #[serde(default)]
    pub alias_type_arguments: Vec<u32>,
}

/// A symbol's wire reference: identity plus its owner context. `id` is the
/// stable per-session identity we use for mutable-root tracking.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolReference {
    pub id: u64,
    #[serde(default)]
    pub kind: u32,
    #[serde(default)]
    pub file: Option<serde_json::Value>,
    #[serde(default)]
    pub snapshot: u64,
    #[serde(default)]
    pub project: String,
}

/// A signature answer (`getSignaturesOfType`): the id is the handle used by
/// `getReturnTypeOfSignature`.
#[derive(Debug, Deserialize)]
pub struct SignatureResponse {
    pub id: u64,
}

/// A symbol answer (`getSymbolAtPosition` and friends).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolResponse {
    pub reference: SymbolReference,
    pub name: String,
    pub flags: u32,
    #[serde(default)]
    pub check_flags: u32,
    #[serde(default)]
    pub declarations: Vec<String>,
    #[serde(default)]
    pub value_declaration: Option<String>,
}

/// Zero-based line and UTF-16 character position in a diagnostic.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPosition {
    pub line: u32,
    pub character: u32,
}

/// One diagnostic from `getSemanticDiagnostics` and friends.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticResponse {
    #[serde(default)]
    pub file_name: Option<String>,
    pub pos: u32,
    pub end: u32,
    #[serde(default)]
    pub start_position: Option<DiagnosticPosition>,
    #[serde(default)]
    pub end_position: Option<DiagnosticPosition>,
    pub code: i32,
    pub category: i32,
    #[serde(default)]
    pub source: Option<String>,
    pub text: String,
}
