//! Wire-protocol smoke test for `tsrs --api`: drives the tuple framing
//! against a scratch project with a real tsrs binary and asserts the
//! response shapes the conditions port relies on. Skips silently when no
//! `tsrs` binary is on PATH. Requires `--features tsrs-inference`.

#![cfg(feature = "tsrs-inference")]

use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use oxvelte::transport::message_pack::{MessagePackReader, MessagePackWriter, MessageType};

const SOURCE: &str = r#"const definitely: number = 42;
const maybe: string | null = definitely > 0 ? "x" : null;
const arr = [1, 2, 3];
if (maybe) {
  console.log(maybe.length);
}
const bad: number = "oops";
"#;

fn request(
    writer: &mut MessagePackWriter<ChildStdin>,
    reader: &mut MessagePackReader<ChildStdout>,
    method: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    let payload = serde_json::to_vec(params).unwrap();
    writer
        .write_tuple(MessageType::Request, method.as_bytes(), &payload)
        .unwrap();
    let tuple = reader.read_tuple().unwrap();
    let text = String::from_utf8_lossy(&tuple.payload).into_owned();
    match tuple.msg_type {
        MessageType::Response => {
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method}: bad JSON: {e}: {text}"))
        }
        MessageType::Error => panic!("server error for {method}: {text}"),
        other => panic!("unexpected tuple {other:?} for {method}: {text}"),
    }
}

fn type_at(
    writer: &mut MessagePackWriter<ChildStdin>,
    reader: &mut MessagePackReader<ChildStdout>,
    snapshot: u64,
    project: &str,
    file: &str,
    needle: &str,
) -> serde_json::Value {
    let position = SOURCE.find(needle).unwrap_or_else(|| panic!("{needle} in SOURCE")) as u32;
    request(
        writer,
        reader,
        "getTypeAtPosition",
        &serde_json::json!({
            "snapshot": snapshot,
            "project": project,
            "file": file,
            "position": position,
        }),
    )
}

#[test]
fn oxvelte_tsrs_communication_smoke_test() {
    if Command::new("tsrs").arg("--version").output().is_err() {
        eprintln!("skipping: no tsrs binary on PATH");
        return;
    }

    let dir = std::env::temp_dir().join("oxvelte-tsrs-smoke-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"noEmit":true}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("a.ts"), SOURCE).unwrap();
    let file = dir.join("a.ts").to_string_lossy().into_owned();
    let tsconfig = dir.join("tsconfig.json").to_string_lossy().into_owned();

    let mut child: Child = Command::new("tsrs")
        .args(["--api", "-cwd"])
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn tsrs --api");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut writer = MessagePackWriter::new(stdin);
    let mut reader = MessagePackReader::new(stdout);
    let w = &mut writer;
    let r = &mut reader;

    let init = request(w, r, "initialize", &serde_json::Value::Null);
    assert!(init.get("caseSensitivity").is_some(), "initialize: {init}");
    assert_eq!(
        init["currentDirectory"].as_str().unwrap(),
        dir.to_string_lossy().as_ref(),
        "initialize currentDirectory"
    );

    let snapshot_resp = request(
        w,
        r,
        "createSnapshot",
        &serde_json::json!({ "openProjects": [tsconfig] }),
    );
    let snapshot = snapshot_resp["snapshot"].as_u64().expect("snapshot id");
    let project = snapshot_resp["projects"][0]["id"]
        .as_str()
        .expect("project id")
        .to_owned();

    // TS 7.1-beta TypeFlags observed: null=0x8, string=0x20, number=0x40,
    // string literal=0x400, union=0x8000000, object=0x100000. These differ
    // from TS <= 5; the conditions port must use TS7 values. Assert on the
    // stable fields rather than the raw bits.
    let number = type_at(w, r, snapshot, &project, &file, "definitely: number");
    assert_eq!(number["intrinsicName"], "number", "number intrinsic: {number}");

    let union = type_at(w, r, snapshot, &project, &file, "maybe: string | null");
    assert!(union["intrinsicName"].is_null(), "union has no intrinsicName: {union}");
    let parts = request(
        w,
        r,
        "getTypesOfType",
        &serde_json::json!({
            "snapshot": snapshot,
            "project": project,
            "objectId": union["id"].as_u64().expect("union type id"),
        }),
    );
    let names: Vec<&str> = parts
        .as_array()
        .expect("union parts")
        .iter()
        .filter_map(|p| p["intrinsicName"].as_str())
        .collect();
    assert_eq!(parts.as_array().unwrap().len(), 2, "union part count: {parts}");
    assert!(
        names.contains(&"null") && names.contains(&"string"),
        "union parts: {names:?}"
    );
    assert!(
        parts.as_array().unwrap().iter().all(|p| p["flags"].is_u64()),
        "parts carry flags: {parts}"
    );
    assert!(
        parts.as_array().unwrap().iter().all(|p| p["id"].is_u64()),
        "parts carry ids: {parts}"
    );

    let literal = type_at(w, r, snapshot, &project, &file, "oops");
    assert_eq!(literal["value"], "oops", "literal value inline: {literal}");
    assert!(literal["freshType"].is_u64(), "literal freshType handle: {literal}");

    let object = type_at(w, r, snapshot, &project, &file, "arr =");
    assert!(object["objectFlags"].is_u64(), "object flags: {object}");

    let diagnostics = request(
        w,
        r,
        "getSemanticDiagnostics",
        &serde_json::json!({ "snapshot": snapshot, "project": project, "files": [file] }),
    );
    let codes: Vec<i64> = diagnostics
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter_map(|d| d["code"].as_i64())
        .collect();
    assert_eq!(codes, [2322], "semantic diagnostics: {diagnostics}");

    drop(writer);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
