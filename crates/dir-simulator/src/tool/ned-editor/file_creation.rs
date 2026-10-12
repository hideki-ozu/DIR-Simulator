//! New input candidates assembled solely from captured project state.
use super::input::{
    CapturedFile, FileRole, ned_package, parse_current_inputs, relative_path, validate_added_file,
};
use super::model::EditorSession;
use super::{EditorError, Result, hash, random_id};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(crate) struct FileCreation {
    pub text: Vec<(String, String)>,
    pub added: Vec<CapturedFile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePayload {
    role: String,
    #[serde(default)]
    root_id: Option<String>,
    relative_path: String,
    template_id: String,
    #[serde(default)]
    declaration: Option<Declaration>,
    format: Format,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    kind: String,
    local_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Format {
    newline: String,
    bom: bool,
}
fn error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-CREATE-FILE", message, 422)
}
fn payload(value: &Value) -> Result<CreatePayload> {
    let p: CreatePayload =
        serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    let (role, kind) = match p.template_id.as_str() {
        "ned.module_empty" => ("ned", Some("module")),
        "ned.network_empty" => ("ned", Some("network")),
        "ned.controller" | "ned.bus" => ("ned", Some("simple")),
        "ned.channel_fixed_delay" => ("ned", Some("channel")),
        "json.workload_empty" => ("workload", None),
        "json.gateway_empty" => ("model_config", None),
        _ => return Err(error("Unknown registered template")),
    };
    if p.role != role || !matches!(p.format.newline.as_str(), "LF" | "CRLF") {
        return Err(error("Template role or format mismatch"));
    }
    relative_path(&p.relative_path, if role == "ned" { "ned" } else { "json" })?;
    if let Some(kind) = kind {
        let root = p
            .root_id
            .as_deref()
            .ok_or_else(|| error("NED root is required"))?;
        let index = root
            .strip_prefix("ned-root-")
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| error("Invalid root ID"))?;
        if root != format!("ned-root-{index}") {
            return Err(error("Invalid root ID"));
        }
        let d = p
            .declaration
            .as_ref()
            .ok_or_else(|| error("NED declaration is required"))?;
        if d.kind != kind
            || !crate::input::identifier(&d.local_name)
            || crate::input::reserved(&d.local_name)
        {
            return Err(error("Template declaration kind or local name mismatch"));
        }
        ned_package(std::path::Path::new(&p.relative_path))?;
    } else if p.root_id.is_some()
        || p.declaration.is_some()
        || value.get("root_id").is_some()
        || value.get("declaration").is_some()
    {
        return Err(error("JSON root is implicit and has no declaration"));
    }
    Ok(p)
}
pub(crate) fn validate_create_payload(value: &Value) -> Result<()> {
    payload(value).map(|_| ())
}

pub(crate) fn plan_create_file(model: &EditorSession, value: &Value) -> Result<FileCreation> {
    let p = payload(value)?;
    parse_current_inputs(&model.project)?;
    let relative = relative_path(
        &p.relative_path,
        if p.role == "ned" { "ned" } else { "json" },
    )?;
    let profile = model
        .project
        .header
        .profile
        .as_deref()
        .unwrap_or("can.cc.ideal.v1");
    let mut text = vec![];
    let (path, role, raw) = if p.role == "ned" {
        let index = p
            .root_id
            .as_ref()
            .unwrap()
            .strip_prefix("ned-root-")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let root = model
            .project
            .header
            .roots
            .get(index)
            .ok_or_else(|| error("Unknown NED root"))?;
        let package = ned_package(&relative)?;
        let d = p.declaration.as_ref().unwrap();
        let body = match p.template_id.as_str() {
            "ned.module_empty" | "ned.network_empty" => {
                format!("{} {} {{}}\n", d.kind, d.local_name)
            }
            _ => {
                if matches!(p.template_id.as_str(), "ned.controller" | "ned.bus")
                    && !matches!(profile, "can.cc.ideal.v1" | "can.cc.multibus.v1")
                {
                    return Err(error(
                        "Standard CAN templates require a supported CAN profile",
                    ));
                }
                let builtin = match p.template_id.as_str() {
                    "ned.controller" => "Controller",
                    "ned.bus" => "Bus",
                    _ => "FixedDelay",
                };
                let (_, body) = super::template::materialize(
                    &format!("@builtin:{builtin}"),
                    &package,
                    profile == "can.cc.multibus.v1",
                    &BTreeSet::new(),
                )?;
                // These standard templates each contain one declaration and no
                // type dependencies; change only its declared local name.
                body.replacen(
                    &format!("{} {builtin} {{", d.kind),
                    &format!("{} {} {{", d.kind, d.local_name),
                    1,
                )
            }
        };
        (
            root.join(&relative),
            FileRole::Ned {
                root_index: index,
                relative,
                package: package.clone(),
            },
            format!("package {package};\n\n{body}"),
        )
    } else {
        let (role, key, rows) = if p.role == "workload" {
            (FileRole::Workload, "workload", "generators")
        } else {
            (FileRole::ModelConfig, "model-config", "gateways")
        };
        if !matches!(profile, "can.cc.ideal.v1" | "can.cc.multibus.v1")
            || (p.role == "model_config" && profile != "can.cc.multibus.v1")
        {
            return Err(error("JSON template is unavailable for the active profile"));
        }
        if model
            .project
            .files
            .values()
            .any(|f| f.roles.contains(&role))
            || (role == FileRole::Workload && model.project.header.workload.is_some())
            || (role == FileRole::ModelConfig && model.project.header.model_config.is_some())
        {
            return Err(error("Singleton JSON role or reference already exists"));
        }
        let config = model
            .project
            .file_by_path(&model.project.config)
            .ok_or_else(|| error("Missing config"))?;
        let quoted = serde_json::to_string(&p.relative_path).map_err(|e| error(e.to_string()))?;
        let ini =
            super::builtin::ini_values(&config.text, &BTreeMap::from([(key.to_owned(), quoted)]))?;
        crate::input::inspect_config(&ini, &model.project.config, &model.project.cwd)
            .map_err(|d| EditorError::common("E-EDITOR-CREATE-FILE", d))?;
        text.push((config.id.clone(), ini));
        (
            model
                .project
                .config
                .parent()
                .ok_or_else(|| error("Config parent missing"))?
                .join(relative),
            role,
            format!("{{\n  \"schema_version\": 1,\n  \"{rows}\": []\n}}\n"),
        )
    };
    let raw = if p.format.newline == "CRLF" {
        raw.replace('\n', "\r\n")
    } else {
        raw
    };
    let raw: Arc<str> = if p.format.bom {
        format!("\u{feff}{raw}").into()
    } else {
        raw.into()
    };
    let digest = hash(raw.as_bytes());
    let file = CapturedFile {
        id: random_id("file-")?,
        path,
        roles: vec![role],
        text: raw.clone(),
        hash: digest.clone(),
        origin_text: raw,
        origin_hash: digest,
        stat: None,
    };
    validate_added_file(&model.project, &file)?;
    let mut candidate = model.project.clone();
    candidate.files.insert(file.id.clone(), file.clone());
    for (id, raw) in &text {
        let f = candidate.files.get_mut(id).unwrap();
        f.text = raw.clone().into();
        f.hash = hash(raw.as_bytes());
    }
    parse_current_inputs(&candidate)?;
    Ok(FileCreation {
        text,
        added: vec![file],
    })
}

#[cfg(test)]
#[path = "file_creation_tests.rs"]
mod tests;
