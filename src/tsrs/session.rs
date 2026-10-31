//! A tsrs checking session for one component: builds the virtual `.ts` file
//! (the port of `typescript-service.cjs`'s construction), spawns a
//! `TsrsClient`, and loads a synthetic program over the virtual file via a
//! layered request filesystem — the component never touches disk.

use std::path::{Path, PathBuf};

use crate::tsrs::resolve;
use crate::tsrs::{TsrsClient, TsrsError};

/// One appended template expression's extent in the virtual source and where
/// it came from in the real source. All offsets are UTF-16 code units.
#[derive(Debug, Clone, Copy)]
pub struct Projection {
    pub start: usize,
    pub end: usize,
    pub original_start: usize,
}

/// A template expression span handed to the session, in UTF-16 offsets.
#[derive(Debug, Clone, Copy)]
pub struct TemplateExpression {
    pub start: usize,
    pub end: usize,
    pub condition: bool,
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Builds the virtual source: every non-script character blanked to spaces
/// (CR/LF preserved, so offsets stay 1:1 with the component), script ranges
/// restored verbatim, and each template expression appended as
/// `if (<expr>) {}` / `void (<expr>);` with a projection back to its origin.
/// All incoming offsets are UTF-16 code units, matching the JS sidecar.
pub fn build_virtual_source(
    source: &str,
    script_ranges: &[(usize, usize)],
    templates: &[TemplateExpression],
) -> (String, Vec<Projection>) {
    let units: Vec<u16> = source.encode_utf16().collect();
    let mut blanked: Vec<u16> = units
        .iter()
        .map(|&u| {
            if u == u16::from(b'\r') || u == u16::from(b'\n') {
                u
            } else {
                u16::from(b' ')
            }
        })
        .collect();
    for &(start, end) in script_ranges {
        assert!(
            start <= end && end <= units.len(),
            "TypeScript script ranges must use valid UTF-16 source offsets."
        );
        blanked[start..end].copy_from_slice(&units[start..end]);
    }
    let mut virtual_units = blanked;
    let mut projections = Vec::with_capacity(templates.len());
    for expression in templates {
        assert!(
            expression.start <= expression.end && expression.end <= units.len(),
            "TypeScript template ranges must use valid UTF-16 source offsets."
        );
        let prefix = if expression.condition {
            utf16("\nif (")
        } else {
            utf16("\nvoid (")
        };
        virtual_units.extend_from_slice(&prefix);
        let start = virtual_units.len();
        virtual_units.extend_from_slice(&units[expression.start..expression.end]);
        projections.push(Projection {
            start,
            end: virtual_units.len(),
            original_start: expression.start,
        });
        let suffix = if expression.condition {
            utf16(") {}")
        } else {
            utf16(");")
        };
        virtual_units.extend_from_slice(&suffix);
    }
    (
        String::from_utf16(&virtual_units).expect("virtual source is valid UTF-16"),
        projections,
    )
}

/// Maps a virtual-source UTF-16 range back to the component source: real
/// script ranges pass through unchanged, appended template expressions map
/// through their projection, and anything else returns `None` (mirrors the
/// sidecar's `range`).
pub fn project_range(
    source_len_utf16: usize,
    projections: &[Projection],
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    if end <= source_len_utf16 {
        return Some((start, end));
    }
    let projection = projections.iter().find(|part| start >= part.start && end <= part.end)?;
    Some((
        projection.original_start + start - projection.start,
        projection.original_start + end - projection.start,
    ))
}

/// Discovers the project's tsconfig: the `settings.typescript.project`
/// override (resolved against the component's directory), else the nearest
/// `tsconfig.json` walked up from the component — mirroring the JS sidecar's
/// `ts.findConfigFile`.
pub fn find_tsconfig(filename: &Path, settings: &serde_json::Value) -> Option<PathBuf> {
    let dir = filename.parent()?;
    if let Some(configured) = settings.get("typescript").and_then(|t| t.get("project")) {
        let configured = configured.as_str()?;
        let resolved = dir.join(configured);
        return resolved.is_file().then_some(resolved);
    }
    for ancestor in dir.ancestors() {
        let candidate = ancestor.join("tsconfig.json");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// A live checking session over one component's virtual file.
pub struct TsrsSession {
    pub client: TsrsClient,
    pub snapshot: u64,
    /// The synthetic program's project handle.
    pub project: String,
    pub virtual_filename: PathBuf,
    pub virtual_source: String,
    /// The virtual source as UTF-16 code units, for span arithmetic against
    /// the encoded AST.
    virtual_units: Vec<u16>,
    pub projections: Vec<Projection>,
    /// The virtual file's canonical path key (lowercased when the server is
    /// case-insensitive), used to construct node handles.
    path_key: String,
    /// Lazily decoded node records for the virtual file.
    node_records: Option<Vec<crate::tsrs::binary_ast::NodeRecord>>,
    /// UTF-16 length of the component source; template expressions live at
    /// offsets beyond it.
    pub source_len_utf16: usize,
    pub strict_null_checks: bool,
}

impl TsrsSession {
    /// Maps a virtual UTF-16 range back to component offsets (see
    /// [`project_range`]).
    pub fn project_range(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        project_range(self.source_len_utf16, &self.projections, start, end)
    }

    /// The type of the expression at an exact UTF-16 span in the virtual
    /// file — the wire equivalent of the sidecar's `checker.getTypeAtLocation`
    /// on an arbitrary expression node. Works for non-token expressions
    /// (calls, property accesses, arrow functions), which position-only
    /// queries cannot address. Resolves the span to a node handle through the
    /// binary AST, then applies `getBaseConstraintOfType` fallback semantics
    /// (the sidecar's `type()`): base constraint when present, else the type.
    pub fn type_at_span(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<Option<crate::tsrs::protocol::TypeResponse>, TsrsError> {
        if self.node_records.is_none() {
            let bytes = self
                .client
                .get_source_file_data(self.snapshot, &self.project, &self.virtual_filename)?;
            self.node_records = Some(
                crate::tsrs::binary_ast::read_nodes(&bytes).ok_or_else(|| TsrsError::Server {
                    method: "getSourceFile".to_owned(),
                    message: "unrecognized binary AST encoding".to_owned(),
                })?,
            );
        }
        let records = self.node_records.as_deref().unwrap_or(&[]);
        let Some(record) =
            crate::tsrs::binary_ast::find_by_span(records, &self.virtual_units, start as u32, end as u32)
        else {
            return Ok(None);
        };
        let handle = format!("{}.{}.{}", record.index, record.kind, self.path_key);
        let Some(ty) = self
            .client
            .get_type_at_location(self.snapshot, &self.project, &handle)?
        else {
            return Ok(None);
        };
        let base = self
            .client
            .get_base_constraint_of_type(self.snapshot, &self.project, ty.id)?;
        Ok(Some(base.unwrap_or(ty)))
    }

    /// Spawns tsrs, loads the project's configuration, and creates a
    /// synthetic program over the virtual file. Mirrors the JS sidecar:
    /// parsed tsconfig options (or strict defaults) with
    /// `noEmit/allowJs/checkJs` forced, project files plus the virtual file
    /// as roots.
    pub fn create(
        filename: &Path,
        source: &str,
        script_ranges: &[(usize, usize)],
        templates: &[TemplateExpression],
        settings: &serde_json::Value,
    ) -> Result<Self, TsrsError> {
        let dir = filename.parent().unwrap_or_else(|| Path::new("."));
        let binary = resolve::find_tsrs_binary(filename).ok_or_else(|| TsrsError::Server {
            method: "resolve".to_owned(),
            message: "no tsrs binary found in the project's node_modules or on PATH".to_owned(),
        })?;
        let (virtual_source, projections) = build_virtual_source(source, script_ranges, templates);
        let virtual_filename = PathBuf::from(format!("{}.ts", filename.to_string_lossy()));
        let source_len_utf16 = source.encode_utf16().count();

        let mut client = TsrsClient::spawn(&binary, dir)?;
        let tsconfig = find_tsconfig(filename, settings);

        // Step 1: open the configured project (if any) to learn its parsed
        // options and file list, exactly as the sidecar reads tsconfig.
        let mut options = serde_json::json!({
            "strict": true, "target": "esnext",
            "module": "esnext", "moduleResolution": "bundler",
        });
        let mut root_files: Vec<String> = Vec::new();
        let mut snapshot = 0u64;
        if let Some(tsconfig) = &tsconfig {
            let resp: crate::tsrs::protocol::CreateSnapshotResponse = client.request(
                "createSnapshot",
                &serde_json::json!({ "openProjects": [tsconfig.to_string_lossy()] }),
            )?;
            snapshot = resp.snapshot;
            let parsed = resp.projects.first().and_then(|p| p.parsed_command_line.as_ref());
            if let Some(parsed) = parsed {
                if !parsed.options.is_null() {
                    options = parsed.options.clone();
                }
                root_files = parsed.file_names.clone();
                let errors: Vec<&serde_json::Value> = parsed
                    .errors
                    .iter()
                    // A Svelte-only project need not have any .ts files on
                    // disk; "no inputs" is not a configuration failure.
                    .filter(|error| error["code"].as_i64() != Some(18003))
                    .collect();
                if !errors.is_empty() {
                    return Err(TsrsError::Server {
                        method: "createSnapshot".to_owned(),
                        message: format!(
                            "Unable to read TypeScript configuration {}: {}",
                            tsconfig.display(),
                            errors
                                .iter()
                                .filter_map(|e| e["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        ),
                    });
                }
            }
        }
        // The sidecar forces these regardless of project configuration.
        options["noEmit"] = serde_json::json!(true);
        options["allowJs"] = serde_json::json!(true);
        options["checkJs"] = serde_json::json!(true);
        let strict_null_checks = match (options["strict"].as_bool(), options["strictNullChecks"].as_bool()) {
            (_, Some(explicit)) => explicit,
            (Some(strict), None) => strict,
            (None, None) => false,
        };

        // Step 2: create the synthetic program over the project files plus
        // the virtual file, supplied through a layered filesystem.
        root_files.push(virtual_filename.to_string_lossy().into_owned());
        let changes = serde_json::json!({
            "createPrograms": [{
                "rootFiles": root_files,
                "compilerOptions": options,
            }],
            "fileSystem": {
                "kind": "layer",
                "files": { virtual_filename.to_string_lossy(): virtual_source },
            },
        });
        let resp: serde_json::Value = if tsconfig.is_some() {
            client.request(
                "updateSnapshot",
                &serde_json::json!({ "snapshot": snapshot, "changes": changes }),
            )?
        } else {
            client.request("createSnapshot", &changes)?
        };
        // Snapshots are immutable: the update returns the NEW snapshot the
        // synthetic program lives in.
        snapshot = resp["snapshot"].as_u64().unwrap_or(snapshot);
        let project = resp["operation"]["createdPrograms"][0]
            .as_str()
            .ok_or_else(|| TsrsError::Server {
                method: "createPrograms".to_owned(),
                message: format!("no synthetic program in response: {resp}"),
            })?
            .to_owned();

        let path_key = {
            let path = virtual_filename.to_string_lossy().into_owned();
            if client.case_sensitivity == 0 {
                path.to_lowercase()
            } else {
                path
            }
        };
        let virtual_units = virtual_source.encode_utf16().collect();

        Ok(Self {
            client,
            snapshot,
            project,
            virtual_filename,
            virtual_source,
            virtual_units,
            projections,
            path_key,
            node_records: None,
            source_len_utf16,
            strict_null_checks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn virtual_source_blanks_templates_and_projects_back() {
        let source = "<script lang=\"ts\">const a = 1;</script>\n{#if a}<p>{a}</p>{/if}";
        let script_start = source.find("const a = 1;").unwrap();
        let test_start = source.find("{#if a}").unwrap() + "{#if ".len();
        let mustache_start = source.rfind('a').unwrap();
        let (virtual_source, projections) = build_virtual_source(
            source,
            &[(script_start, script_start + "const a = 1;".len())],
            &[
                TemplateExpression {
                    start: test_start,
                    end: test_start + 1,
                    condition: true,
                },
                TemplateExpression {
                    start: mustache_start,
                    end: mustache_start + 1,
                    condition: false,
                },
            ],
        );
        assert!(virtual_source.contains("const a = 1;"));
        assert!(!virtual_source.contains("<p>"), "template markup must be blanked");
        assert!(virtual_source.ends_with("\nif (a) {}\nvoid (a);"));
        // Offsets are 1:1 with the source for the unblanked prefix.
        assert_eq!(virtual_source.len(), source.len() + "\nif (a) {}\nvoid (a);".len());
        // The appended `a` in `if (a)` projects back to the {#if} test.
        let if_pos = virtual_source.find("\nif (").unwrap() + 5;
        assert_eq!(
            project_range(source.len(), &projections, if_pos, if_pos + 1),
            Some((test_start, test_start + 1))
        );
        // Real-source ranges pass through.
        assert_eq!(
            project_range(source.len(), &projections, script_start, script_start + 1),
            Some((script_start, script_start + 1))
        );
    }

    #[test]
    fn session_checks_a_virtual_component() {
        let Some(binary) = resolve::find_on_path() else {
            eprintln!("skipping: no tsrs binary on PATH");
            return;
        };
        let _ = &binary;
        let dir = std::env::temp_dir().join("oxvelte-tsrs-session-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();

        let source = concat!(
            "<script lang=\"ts\">\n",
            "const maybe: string | null = Math.random() > 0.5 ? \"x\" : null;\n",
            "</script>\n",
            "{#if maybe}<p>{maybe}</p>{/if}\n"
        );
        let script_start = source.find("const maybe").unwrap();
        let script_end = source.find("null;").unwrap() + "null;".len();
        let test_start = source.find("maybe}").unwrap();
        let filename = dir.join("Component.svelte");

        let mut session = TsrsSession::create(
            &filename,
            source,
            &[(script_start, script_end)],
            &[TemplateExpression {
                start: test_start,
                end: test_start + 5,
                condition: true,
            }],
            &serde_json::json!({}),
        )
        .expect("session");

        assert!(session.strict_null_checks, "strict tsconfig implies strictNullChecks");
        assert!(session.snapshot != 0);
        assert!(!session.project.is_empty());

        // The type of `maybe` inside the appended `if (maybe) {}` is the
        // union string | null.
        let union = session
            .client
            .get_type_at_position(
                session.snapshot,
                &session.project,
                &session.virtual_filename,
                session.projections[0].start as u32,
            )
            .expect("getTypeAtPosition");
        let parts = session
            .client
            .get_types_of_type(session.snapshot, &session.project, union.id)
            .expect("getTypesOfType");
        let names: Vec<&str> = parts.iter().filter_map(|p| p.intrinsic_name.as_deref()).collect();
        assert!(names.contains(&"string") && names.contains(&"null"), "parts: {names:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn type_at_span_resolves_whole_expressions() {
        let dir = std::env::temp_dir().join("oxvelte-tsrs-span-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#).unwrap();

        // `Math.random()` and `obj.a` are not single tokens: only a
        // span-addressed query returns their types.
        let source = concat!(
            "<script lang=\"ts\">\n",
            "const obj = { a: 1 };\n",
            "const r = Math.random();\n",
            "const v = obj.a;\n",
            "</script>\n"
        );
        let script_start = source.find("const obj").unwrap();
        let script_end = source.find("const v = obj.a;").unwrap() + "const v = obj.a;".len();
        let filename = dir.join("Component.svelte");

        let mut session = TsrsSession::create(
            &filename,
            source,
            &[(script_start, script_end)],
            &[],
            &serde_json::json!({}),
        )
        .expect("session");

        for (needle, expected) in [("Math.random()", "number"), ("obj.a;", "number")] {
            let span_start = source.find(needle).unwrap();
            let span_end = span_start + needle.trim_end_matches(';').len();
            let ty = session
                .type_at_span(span_start, span_end)
                .unwrap_or_else(|e| panic!("type_at_span({needle}): {e}"))
                .unwrap_or_else(|| panic!("no type for {needle}"));
            assert_eq!(ty.intrinsic_name.as_deref(), Some(expected), "type of {needle}: {ty:?}");
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
