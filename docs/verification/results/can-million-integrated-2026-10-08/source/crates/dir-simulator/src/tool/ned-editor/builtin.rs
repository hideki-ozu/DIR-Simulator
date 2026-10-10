//! Copy embedded definitions into ordinary project sources in one undoable edit.
use super::analysis::{self, shapes};
use super::input::{CapturedFile, FileRole};
use super::model::EditorSession;
use super::source_index::{self, Patch, SourceIndex};
use super::{EditorError, Result, hash, random_id};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-TEMPLATE", message, 422)
}
fn package(file: &CapturedFile) -> &str {
    file.roles
        .iter()
        .find_map(|role| match role {
            FileRole::Ned { package, .. } => Some(package.as_str()),
            _ => None,
        })
        .unwrap_or("")
}
fn stage_text(model: &mut EditorSession, id: &str, text: String) -> Result<()> {
    let file = model
        .project
        .files
        .get_mut(id)
        .ok_or_else(|| error("Missing source"))?;
    if file.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })) {
        let parsed = crate::input::parse_ned(&text, &file.path, package(file))
            .map_err(|d| EditorError::common("E-EDITOR-TEMPLATE", d))?;
        model.project.parsed.insert(id.into(), parsed);
        model.parsed_hashes.insert(id.into(), hash(text.as_bytes()));
        model.syntax.insert(id.into(), "parsed".into());
    }
    file.hash = hash(text.as_bytes());
    file.text = text.into();
    Ok(())
}

/// Replace only INI value spans and add missing General keys before Channel sections.
pub(crate) fn ini_values(raw: &str, changes: &BTreeMap<String, String>) -> Result<String> {
    let mut patches = vec![];
    let mut remaining = changes.clone();
    let mut offset = 0;
    let mut end_general = raw.len();
    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        if trimmed.starts_with("[Channel ") {
            end_general = offset;
            break;
        }
        if let Some((left, right)) = line.split_once('=') {
            let key = left.trim();
            if let Some(replacement) = remaining.remove(key) {
                let start = offset + left.len() + 1 + right.len() - right.trim_start().len();
                let end = offset + left.len() + 1 + right.trim_end().len();
                patches.push(Patch {
                    span: start..end,
                    replacement,
                });
            }
        }
        offset += line.len();
    }
    if !remaining.is_empty() {
        let nl = if raw.contains("\r\n") { "\r\n" } else { "\n" };
        let mut insertion = String::new();
        if end_general > 0 && !raw[..end_general].ends_with('\n') {
            insertion.push_str(nl);
        }
        for (key, value) in remaining {
            insertion.push_str(&format!("{key} = {value}{nl}"));
        }
        patches.push(Patch {
            span: end_general..end_general,
            replacement: insertion,
        });
    }
    source_index::apply(raw, patches)
}

fn migrate_multibus(model: &mut EditorSession) -> Result<()> {
    if model.project.header.profile.as_deref() == Some("can.cc.multibus.v1") {
        return Ok(());
    }
    let network = model
        .project
        .header
        .network
        .clone()
        .ok_or_else(|| error("Network is not specified"))?;
    let mut stack = vec![(
        network.rsplit('.').next().unwrap().to_owned(),
        network,
        BTreeSet::new(),
    )];
    let mut buses = BTreeSet::new();
    let mut count = 0;
    while let Some((path, name, mut ancestors)) = stack.pop() {
        count += 1;
        if count > 10_000 || ancestors.len() >= 128 || !ancestors.insert(name.clone()) {
            return Err(error(
                "Network expansion is cyclic or exceeds the migration limit",
            ));
        }
        let declaration = model.unique_type(&name)?;
        if declaration.implementation.as_deref() == Some("dir.can.Bus") {
            buses.insert(path.clone());
        }
        for (child, ty) in declaration.children {
            stack.push((format!("{path}.{child}"), ty, ancestors.clone()));
        }
    }
    for file in model.project.ned_files().cloned().collect::<Vec<_>>() {
        let parsed = model.current_parse(&file.id)?;
        let mut expected = shapes(parsed);
        let indexes = SourceIndex::build(&file.text, parsed)?;
        let mut patches = vec![];
        for (ordinal, declaration) in expected.iter_mut().enumerate() {
            let (old, new) = match declaration.implementation.as_deref() {
                Some("dir.can.Controller") => ("dir.can.Controller", "dir.can.MultibusController"),
                Some("dir.can.Bus") => ("dir.can.Bus", "dir.can.MultibusBus"),
                _ => continue,
            };
            let sections = &indexes.declarations[ordinal].sections;
            let parameters = sections
                .get("parameters")
                .ok_or_else(|| error("No implementation source index"))?;
            let implementation = parameters
                .statements
                .iter()
                .flat_map(|s| &s.tokens)
                .find(|t| t.text == format!("\"{old}\""))
                .ok_or_else(|| error("Implementation literal cannot be mapped"))?;
            patches.push(Patch {
                span: implementation.span.clone(),
                replacement: format!("\"{new}\""),
            });
            declaration.implementation = Some(new.into());
            if old == "dir.can.Bus" {
                if let Some(profile) = declaration.parameters.get_mut("profile") {
                    if profile.default.as_deref() == Some("\"can.cc.ideal.v1\"") {
                        let literal = parameters
                            .statements
                            .iter()
                            .find(|s| s.tokens.get(1).is_some_and(|t| t.text == "profile"))
                            .and_then(|s| s.tokens.iter().find(|t| t.text == "\"can.cc.ideal.v1\""))
                            .ok_or_else(|| error("Bus profile literal cannot be mapped"))?;
                        patches.push(Patch {
                            span: literal.span.clone(),
                            replacement: "\"can.cc.multibus.v1\"".into(),
                        });
                        profile.default = Some("\"can.cc.multibus.v1\"".into());
                    }
                }
            }
        }
        if !patches.is_empty() {
            let candidate = source_index::apply(&file.text, patches)?;
            source_index::check_candidate(&candidate, &file.path, package(&file), &expected)?;
            stage_text(model, &file.id, candidate)?;
        }
    }
    let config = model
        .project
        .file_by_path(&model.project.config)
        .ok_or_else(|| error("Missing INI"))?
        .clone();
    let mut changes = BTreeMap::from([("model-profile".into(), "\"can.cc.multibus.v1\"".into())]);
    for (key, value) in &model.project.header.general {
        if key
            .strip_suffix(".profile")
            .is_some_and(|path| buses.contains(path))
            && value == "\"can.cc.ideal.v1\""
        {
            changes.insert(key.clone(), "\"can.cc.multibus.v1\"".into());
        }
    }
    if model.project.header.model_config.is_none() {
        let basename = format!("{}.json", random_id("editor-routing-")?);
        let path = config
            .path
            .parent()
            .ok_or_else(|| error("INI parent missing"))?
            .join(&basename);
        let text: std::sync::Arc<str> =
            "{\n  \"schema_version\": 1,\n  \"gateways\": []\n}\n".into();
        let id = format!("file-{}", hash(path.to_string_lossy().as_bytes()));
        let digest = hash(text.as_bytes());
        model.project.files.insert(
            id.clone(),
            CapturedFile {
                id,
                path,
                roles: vec![FileRole::ModelConfig],
                text: text.clone(),
                hash: digest.clone(),
                origin_text: text,
                origin_hash: digest,
                stat: None,
            },
        );
        changes.insert("model-config".into(), format!("\"{basename}\""));
    }
    stage_text(model, &config.id, ini_values(&config.text, &changes)?)?;
    model.project.refresh_inputs();
    Ok(())
}

impl EditorSession {
    pub fn builtin_command(&mut self, kind: &str, payload: &Value) -> Result<bool> {
        let mut staged = self.clone();
        let mut command = payload.clone();
        let fields = ["type_name", "channel", "tx_channel", "rx_channel"];
        if fields.iter().any(|key| {
            command[*key]
                .as_str()
                .is_some_and(super::template::requires_multibus)
        }) {
            migrate_multibus(&mut staged)?;
        }
        let key = command["parent_type"]
            .as_str()
            .or_else(|| {
                command["connection_key"]
                    .as_str()
                    .and_then(|key| key.split("::connection::").next())
            })
            .ok_or_else(|| error("Missing parent type"))?;
        let (file_id, _, _) = staged.declaration(key)?;
        let mut materialized: BTreeMap<String, String> = BTreeMap::new();
        for field in fields {
            let Some(id) = command[field]
                .as_str()
                .filter(|id| id.starts_with("@builtin:"))
                .map(str::to_owned)
            else {
                continue;
            };
            let name = if let Some(name) = materialized.get(&id) {
                name.clone()
            } else {
                let file = staged.project.files[&file_id].clone();
                let names: BTreeSet<_> = staged
                    .project
                    .ned_files()
                    .map(|f| staged.current_parse(&f.id).map(shapes))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .map(|d| d.name)
                    .collect();
                let (name, definition) = super::template::materialize(
                    &id,
                    package(&file),
                    staged.project.header.profile.as_deref() == Some("can.cc.multibus.v1"),
                    &names,
                )?;
                let nl = if file.text.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                let mut raw = file.text.to_string();
                if !raw.ends_with('\n') {
                    raw.push_str(nl);
                }
                raw.push_str(nl);
                raw.push_str(&definition.replace('\n', nl));
                stage_text(&mut staged, &file_id, raw)?;
                materialized.insert(id.clone(), name.clone());
                name
            };
            command[field] = name.into();
        }
        staged.graph_command(kind, &command)?;
        let text = staged
            .project
            .files
            .values()
            .filter_map(|f| {
                self.project
                    .files
                    .get(&f.id)
                    .filter(|old| old.hash != f.hash)
                    .map(|_| (f.id.clone(), f.text.to_string()))
            })
            .collect();
        let added = staged
            .project
            .files
            .values()
            .filter(|f| !self.project.files.contains_key(&f.id))
            .cloned()
            .collect();
        let layouts = staged
            .project
            .layouts
            .iter()
            .filter(|(id, l)| {
                self.project
                    .layouts
                    .get(*id)
                    .is_some_and(|old| old.adopted != l.adopted)
            })
            .map(|(id, l)| (id.clone(), l.adopted.clone()))
            .collect();
        let changed = self.commit_with_files(text, layouts, added)?;
        self.adopt_analysis(analysis::parse_project(&self.project), self.context_epoch);
        Ok(changed)
    }
}
