//! Previewed edits over captured inputs. Publication is one existing model transaction.
use super::analysis::{parse_project, prepare, type_key};
use super::input::{CapturedFile, ProjectSnapshot};
use super::layout::LayoutDocument;
use super::model::EditorSession;
use super::{EditorError, Result, hash, random_id};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::time::{Duration, Instant};

pub(crate) const PLAN_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const PLAN_TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Default)]
pub(crate) struct EditChanges {
    pub text: Vec<(String, String)>,
    pub added: Vec<CapturedFile>,
    pub layouts: Vec<(String, Option<LayoutDocument>)>,
    pub symbol_remap: Vec<Value>,
    pub instance_remap: Vec<Value>,
    pub semantic_evidence: Value,
}

#[derive(Clone)]
pub(crate) struct EditPlan {
    pub id: String,
    pub digest: String,
    pub created: Instant,
    pub revision: u64,
    pub input_revision: u64,
    pub context_epoch: u64,
    pub snapshot: String,
    pub input_digest: String,
    pub operation: String,
    pub changes: EditChanges,
    pub summary: Value,
    pub impacts: Vec<Value>,
    expected_input_digest: String,
}

fn error(code: &str, message: impl Into<String>, status: u16) -> EditorError {
    EditorError::new(code, message, status)
}
pub(crate) fn strict_fields(v: &Value, required: &[&str], optional: &[&str]) -> Result<()> {
    let object = v
        .as_object()
        .ok_or_else(|| error("E-EDITOR-REQUEST", "Expected an object", 400))?;
    if let Some(key) = object
        .keys()
        .find(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(error(
            "E-EDITOR-REQUEST",
            format!("Unknown field: {key}"),
            400,
        ));
    }
    if let Some(key) = required.iter().find(|key| !object.contains_key(**key)) {
        return Err(error(
            "E-EDITOR-REQUEST",
            format!("Missing field: {key}"),
            400,
        ));
    }
    Ok(())
}
pub(crate) fn text<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| error("E-EDITOR-REQUEST", format!("{name} must be a string"), 400))
}
pub(crate) fn validate_preview_payload(v: &Value) -> Result<()> {
    strict_fields(v, &["operation", "operation_payload"], &[])?;
    match text(v, "operation")? {
        "create_file" => {
            let payload = &v["operation_payload"];
            strict_fields(
                payload,
                &["role", "relative_path", "template_id", "format"],
                &["root_id", "declaration"],
            )?;
            for name in ["role", "relative_path", "template_id"] {
                text(payload, name)?;
            }
            if payload.get("root_id").is_some() {
                text(payload, "root_id")?;
            }
            strict_fields(&payload["format"], &["newline", "bom"], &[])?;
            text(&payload["format"], "newline")?;
            if !payload["format"]["bom"].is_boolean() {
                return Err(error("E-EDITOR-REQUEST", "bom must be a boolean", 400));
            }
            if let Some(declaration) = payload.get("declaration") {
                strict_fields(declaration, &["kind", "local_name"], &[])?;
                text(declaration, "kind")?;
                text(declaration, "local_name")?;
            }
            super::file_creation::validate_create_payload(payload)
        }
        "rename_symbols" => {
            super::refactor_symbols::validate_rename_payload(&v["operation_payload"])
        }
        "detach_instance_type" => {
            super::refactor_symbols::validate_detach_payload(&v["operation_payload"])
        }
        _ => Err(error("E-EDITOR-REQUEST", "Unknown edit operation", 400)),
    }
}
pub(crate) fn validate_apply_payload(v: &Value) -> Result<()> {
    strict_fields(v, &["plan_id", "plan_digest"], &[])?;
    text(v, "plan_id")?;
    text(v, "plan_digest")?;
    Ok(())
}

struct DigestWriter {
    digest: Sha256,
    written: usize,
}
impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self
            .written
            .checked_add(bytes.len())
            .filter(|n| *n <= PLAN_BYTES)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Edit plan exceeds 64 MiB")
            })?;
        self.digest.update(bytes);
        self.written = written;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn plan_digest(value: &Value) -> Result<String> {
    let mut writer = DigestWriter {
        digest: Sha256::new(),
        written: 0,
    };
    writer.digest.update(b"ned-editor-edit-plan-v1\0");
    serde_json::to_writer(&mut writer, value)
        .map_err(|e| error("E-EDITOR-PLAN-LIMIT", e.to_string(), 413))?;
    Ok(format!("{:x}", writer.digest.finalize()))
}
fn changes_fingerprint(model: &EditorSession, changes: &EditChanges) -> Value {
    json!({
        "text":changes.text.iter().map(|(id,text)|json!({"id":id,"before_hash":model.project.files.get(id).map(|file|&file.hash),"after_hash":hash(text.as_bytes()),"after_bytes":text.len().to_string()})).collect::<Vec<_>>(),
        "added":changes.added.iter().map(|file|json!({"id":file.id,"path":file.path,"roles":file.roles,"hash":file.hash,"actual_hash":hash(file.text.as_bytes()),"origin_hash":file.origin_hash,"actual_origin_hash":hash(file.origin_text.as_bytes()),"stat":file.stat})).collect::<Vec<_>>(),
        "layouts":changes.layouts,
        "symbol_remap":changes.symbol_remap,"instance_remap":changes.instance_remap,"semantic_evidence":changes.semantic_evidence
    })
}

fn analyzed(mut model: EditorSession) -> Result<EditorSession> {
    let batch = parse_project(&model.project);
    if let Some(diagnostic) = batch
        .files
        .values()
        .find_map(|file| file.result.as_ref().err())
    {
        return Err(EditorError::common("E-EDITOR-SYNTAX", diagnostic.clone()));
    }
    model.adopt_analysis(batch, model.context_epoch);
    Ok(model)
}
fn apply_changes(
    model: &EditorSession,
    changes: &EditChanges,
    require_ready: bool,
) -> Result<EditorSession> {
    let mut candidate = model.clone();
    if !candidate.commit_with_files(
        changes.text.clone(),
        changes.layouts.clone(),
        changes.added.clone(),
    )? {
        return Err(error("E-EDITOR-NO-CHANGE", "The plan has no changes", 422));
    }
    candidate = analyzed(candidate)?;
    match prepare(&candidate.project) {
        Ok(prepared) => {
            candidate.prepared = Some(std::sync::Arc::new(prepared));
            candidate.analysis = "ready".into();
        }
        Err(diagnostic) if !require_ready => {
            candidate.prepared = None;
            candidate.analysis = "semantic_error".into();
            candidate.diagnostics.push(json!({"origin":"semantic","code":diagnostic.code,"message":diagnostic.message,"severity":"error","snapshot_id":candidate.project.id,"input_revision":candidate.input_revision.to_string()}));
        }
        Err(diagnostic) => return Err(EditorError::common("E-EDITOR-SEMANTIC", diagnostic)),
    }
    Ok(candidate)
}

impl EditPlan {
    pub fn build(model: &EditorSession, request: &Value) -> Result<Self> {
        validate_preview_payload(request)?;
        let model = analyzed(model.clone())?;
        let operation = text(request, "operation")?.to_owned();
        let payload = &request["operation_payload"];
        let changes = match operation.as_str() {
            "create_file" => {
                let creation = super::file_creation::plan_create_file(&model, payload)?;
                EditChanges {
                    text: creation.text,
                    added: creation.added,
                    semantic_evidence: json!({"contract":"constructive_file_addition"}),
                    ..EditChanges::default()
                }
            }
            "rename_symbols" => super::refactor_symbols::plan_rename(&model, payload)?,
            "detach_instance_type" => super::refactor_symbols::plan_detach(&model, payload)?,
            _ => unreachable!("Validated operation"),
        };
        Self::from_changes(&model, operation, payload.clone(), changes)
    }
    pub fn from_changes(
        model: &EditorSession,
        operation: String,
        payload: Value,
        changes: EditChanges,
    ) -> Result<Self> {
        if changes.added.len() > 64 {
            return Err(error(
                "E-EDITOR-PLAN-LIMIT",
                "At most 64 new files per plan",
                413,
            ));
        }
        let candidate = apply_changes(model, &changes, operation != "create_file")?;
        let mut impacts = Vec::new();
        for (id, after) in &changes.text {
            let before = model
                .project
                .files
                .get(id)
                .ok_or_else(|| error("E-EDITOR-UNMAPPED", "Unknown source file", 422))?;
            if before.text.as_ref() != after {
                impacts.push(json!({"kind":"source","file_id":id,"path":before.path,"before_hash":before.hash,"after_hash":hash(after.as_bytes()),"before":before.text.as_ref(),"after":after}));
            }
        }
        for file in &changes.added {
            impacts.push(json!({"kind":"added_file","file_id":file.id,"path":file.path,"roles":file.roles,"before":null,"after":file.text.as_ref(),"after_hash":file.hash}));
        }
        for (id, after) in &changes.layouts {
            let before = model
                .project
                .layouts
                .get(id)
                .and_then(|capture| capture.adopted.as_ref());
            let path = model
                .project
                .files
                .get(id)
                .or_else(|| changes.added.iter().find(|file| file.id == *id))
                .map(|file| &file.path);
            impacts.push(
                json!({"kind":"layout","file_id":id,"path":path,"before":before,"after":after}),
            );
        }
        let input_digest = model.project.input_digest();
        let summary = json!({"schema_version":1,"snapshot_id":model.project.id,"input_digest":input_digest,"base_revision":model.revision.to_string(),"input_revision":model.input_revision.to_string(),"context_epoch":model.context_epoch.to_string(),"operation":operation,"normalized_payload":payload,"added_files":changes.added.iter().map(|file| json!({"id":file.id,"path":file.path,"roles":file.roles,"hash":file.hash})).collect::<Vec<_>>(),"symbol_remap":changes.symbol_remap,"instance_remap":changes.instance_remap,"semantic_evidence":changes.semantic_evidence,"save_shape_effect":if changes.added.is_empty(){"unchanged"}else{"new_save_as_required"},"analysis":candidate.analysis,"diagnostics":candidate.diagnostics,"impact_count":impacts.len().to_string(),"expires_in_ms":"600000"});
        let digest = plan_digest(
            &json!({"summary":summary,"impacts":impacts,"changes":changes_fingerprint(model,&changes)}),
        )?;
        Ok(Self {
            id: random_id("plan-")?,
            digest,
            created: Instant::now(),
            revision: model.revision,
            input_revision: model.input_revision,
            context_epoch: model.context_epoch,
            snapshot: model.project.id.clone(),
            input_digest,
            operation,
            changes,
            summary,
            impacts,
            expected_input_digest: candidate.project.input_digest(),
        })
    }
    pub fn matches(&self, model: &EditorSession) -> bool {
        self.revision == model.revision
            && self.input_revision == model.input_revision
            && self.context_epoch == model.context_epoch
            && self.snapshot == model.project.id
            && self.input_digest == model.project.input_digest()
    }
    pub fn apply(&self, model: &EditorSession) -> Result<EditorSession> {
        if self.created.elapsed() >= PLAN_TTL {
            return Err(error("E-EDITOR-EXPIRED", "Edit plan expired", 410));
        }
        if !self.matches(model) {
            return Err(error(
                "E-EDITOR-REVISION",
                "Edit plan no longer matches the model",
                409,
            ));
        }
        let digest = plan_digest(
            &json!({"summary":self.summary,"impacts":self.impacts,"changes":changes_fingerprint(model,&self.changes)}),
        )?;
        if digest != self.digest {
            return Err(error("E-EDITOR-PLAN", "Edit plan digest changed", 409));
        }
        let candidate = apply_changes(model, &self.changes, self.operation != "create_file")?;
        if candidate.project.input_digest() != self.expected_input_digest {
            return Err(error(
                "E-EDITOR-PLAN",
                "Candidate input changed since preview",
                409,
            ));
        }
        Ok(candidate)
    }
    pub fn page(&self, offset: usize) -> Result<Value> {
        if offset > self.impacts.len() {
            return Err(error("E-EDITOR-REQUEST", "Invalid plan cursor", 400));
        }
        let end = offset.saturating_add(200).min(self.impacts.len());
        let mut value = self.summary.clone();
        value["plan_id"] = self.id.clone().into();
        value["plan_digest"] = self.digest.clone().into();
        value["impacts"] = json!(&self.impacts[offset..end]);
        value["next_cursor"] = if end < self.impacts.len() {
            json!(format!("{}:{end}", self.digest))
        } else {
            Value::Null
        };
        Ok(value)
    }
}

pub(crate) fn symbol_id(
    project: &ProjectSnapshot,
    qname: &str,
    kind: &str,
    name: &str,
) -> Result<String> {
    for file in project.ned_files() {
        if let Some(parsed) = project.parsed.get(&file.id) {
            if let Some(ordinal) = parsed
                .declarations()
                .iter()
                .position(|declaration| declaration.name() == qname)
            {
                return Ok(format!(
                    "symbol-{}",
                    hash(
                        serde_json::to_string(&(
                            file.id.as_str(),
                            ordinal,
                            file.hash.as_str(),
                            kind,
                            name
                        ))
                        .expect("String tuple serializes")
                        .as_bytes()
                    )
                ));
            }
        }
    }
    Err(error(
        "E-EDITOR-UNMAPPED",
        "Unknown declaration identity",
        422,
    ))
}
pub(crate) fn instance_id(project: &ProjectSnapshot, path: &str) -> String {
    instance_id_from_digest(&project.input_digest(), path)
}
pub(crate) fn instance_id_from_digest(input_digest: &str, path: &str) -> String {
    format!(
        "instance-{}",
        hash(format!("{input_digest}\0{path}").as_bytes())
    )
}
pub(crate) fn symbols(model: &EditorSession) -> Vec<Value> {
    let mut result = Vec::new();
    for file in model.project.ned_files() {
        let Ok(parsed) = model.current_parse(&file.id) else {
            continue;
        };
        for (ordinal, declaration) in parsed.declarations().iter().enumerate() {
            let owner = declaration.name();
            let mut names = vec![("type", owner.rsplit('.').next().unwrap_or(owner))];
            names.extend(
                declaration
                    .children()
                    .iter()
                    .map(|(name, _)| ("child", name.as_str())),
            );
            names.extend(
                declaration
                    .gates()
                    .keys()
                    .map(|name| ("gate", name.as_str())),
            );
            names.extend(
                declaration
                    .parameters()
                    .keys()
                    .map(|name| ("parameter", name.as_str())),
            );
            for (kind, name) in names {
                let id = format!(
                    "symbol-{}",
                    hash(
                        serde_json::to_string(&(
                            file.id.as_str(),
                            ordinal,
                            file.hash.as_str(),
                            kind,
                            name
                        ))
                        .expect("String tuple serializes")
                        .as_bytes()
                    )
                );
                result.push(json!({"id":id,"owner":owner,"kind":kind,"name":name,"file_id":file.id,"type_key":type_key(&file.id,ordinal,owner)}));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_private_layout_candidate_cannot_bypass_confirmation_digest() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let project =
            super::super::input::load_project(&root.join("examples/can/baseline.ini"), &root)
                .unwrap();
        let model = EditorSession::new(project);
        let file = model.project.ned_files().next().unwrap();
        let mut layout = LayoutDocument::empty(
            file.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        let mut ty = super::super::layout::TypeLayout::default();
        ty.nodes
            .insert("a".into(), super::super::layout::Position::new(10., 20.));
        layout.types.insert("demo.Main".into(), ty);
        let mut plan = EditPlan::from_changes(
            &model,
            "rename_symbols".into(),
            json!({"entries":[]}),
            EditChanges {
                layouts: vec![(file.id.clone(), Some(layout))],
                ..EditChanges::default()
            },
        )
        .unwrap();
        let before = model.project.input_digest();
        plan.changes.layouts[0]
            .1
            .as_mut()
            .unwrap()
            .types
            .get_mut("demo.Main")
            .unwrap()
            .nodes
            .get_mut("a")
            .unwrap()
            .x = 30.;
        assert_eq!(plan.apply(&model).err().unwrap().code, "E-EDITOR-PLAN");
        assert_eq!(model.project.input_digest(), before);
        assert!(!model.can_undo());
    }
}
