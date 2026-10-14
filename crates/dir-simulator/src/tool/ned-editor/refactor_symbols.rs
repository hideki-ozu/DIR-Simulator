//! Resolved symbol refactors over immutable editor inputs.
#[cfg(test)]
#[path = "acceptance_refactor_tests.rs"]
mod acceptance_refactor_tests;
mod detach;
mod semantic;
#[cfg(test)]
mod tests;
use super::input::{CapturedFile, FileRole, ProjectSnapshot, parse_current_inputs};
use super::model::EditorSession;
use super::refactor::{EditChanges, instance_id, symbol_id};
use super::{EditorError, Result};
use crate::input::{
    ReferenceInspection, ReferenceMemberKind, ReferenceSyntaxKind, ReferenceTarget,
    inspect_references_with_source,
};
use crate::types::SourceSpan;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Symbol {
    owner: String,
    kind: String,
    name: String,
}
impl Symbol {
    fn from_target(target: &ReferenceTarget) -> Option<Self> {
        match target {
            ReferenceTarget::Type { qname } => Some(Self {
                owner: qname.clone(),
                kind: "type".into(),
                name: local(qname).into(),
            }),
            ReferenceTarget::Member {
                owner,
                member_kind,
                name,
            } => Some(Self {
                owner: owner.clone(),
                kind: match member_kind {
                    ReferenceMemberKind::Child => "child",
                    ReferenceMemberKind::Gate => "gate",
                    ReferenceMemberKind::Parameter => "parameter",
                }
                .into(),
                name: name.clone(),
            }),
            _ => None,
        }
    }
}
#[derive(Clone, Default)]
struct Names {
    symbols: BTreeMap<Symbol, String>,
}
impl Names {
    fn name(&self, owner: &str, kind: &str, name: &str) -> String {
        self.symbols
            .get(&Symbol {
                owner: owner.into(),
                kind: kind.into(),
                name: name.into(),
            })
            .cloned()
            .unwrap_or_else(|| name.into())
    }
    fn qname(&self, qname: &str) -> String {
        if let Some(name) = self.symbols.get(&Symbol {
            owner: qname.into(),
            kind: "type".into(),
            name: local(qname).into(),
        }) {
            format!("{}.{}", package(qname), name)
        } else {
            qname.into()
        }
    }
    fn target(&self, target: &ReferenceTarget) -> Option<String> {
        Symbol::from_target(target).map(|s| self.name(&s.owner, &s.kind, &s.name))
    }
    fn decoded(&self, value: &str, segments: &[crate::input::ReferenceSegment]) -> Result<String> {
        let mut result = value.to_owned();
        for s in segments.iter().rev() {
            if s.start_byte > s.end_byte
                || s.end_byte > value.len()
                || !value.is_char_boundary(s.start_byte)
                || !value.is_char_boundary(s.end_byte)
            {
                return Err(error("Invalid decoded reference range"));
            }
            if let Some(name) = self.target(&s.target) {
                result.replace_range(s.start_byte..s.end_byte, &name);
            }
        }
        Ok(result)
    }
}
fn local(qname: &str) -> &str {
    qname.rsplit('.').next().unwrap_or(qname)
}
fn package(qname: &str) -> &str {
    qname.rsplit_once('.').map_or("", |(p, _)| p)
}
fn error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-REFACTOR", message, 422)
}
fn request_error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-REQUEST", message, 400)
}
fn valid_name(name: &str) -> Result<()> {
    if crate::input::identifier(name) && !crate::input::reserved(name) {
        Ok(())
    } else {
        Err(error("Expected a nonreserved local identifier"))
    }
}
fn inspect(project: &ProjectSnapshot) -> Result<ReferenceInspection> {
    let inspected =
        inspect_references_with_source(&project.config, &project.cwd, &project.source())
            .map_err(|d| EditorError::common("E-EDITOR-SEMANTIC", d))?;
    if !inspected.unsupported.is_empty() {
        return Err(error(format!(
            "Reference profile unsupported: {}",
            inspected.prepared.common.profile
        )));
    }
    Ok(inspected)
}
fn catalog(model: &EditorSession) -> Result<BTreeMap<String, Symbol>> {
    let mut result = BTreeMap::new();
    for file in model.project.ned_files() {
        let parsed = model.current_parse(&file.id)?;
        for d in parsed.declarations() {
            let mut symbols = vec![Symbol {
                owner: d.name().into(),
                kind: "type".into(),
                name: local(d.name()).into(),
            }];
            symbols.extend(d.children().iter().map(|(n, _)| Symbol {
                owner: d.name().into(),
                kind: "child".into(),
                name: n.clone(),
            }));
            symbols.extend(d.gates().keys().map(|n| Symbol {
                owner: d.name().into(),
                kind: "gate".into(),
                name: n.clone(),
            }));
            symbols.extend(d.parameters().keys().map(|n| Symbol {
                owner: d.name().into(),
                kind: "parameter".into(),
                name: n.clone(),
            }));
            for symbol in symbols {
                result.insert(
                    symbol_id(&model.project, &symbol.owner, &symbol.kind, &symbol.name)?,
                    symbol,
                );
            }
        }
    }
    Ok(result)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenamePayload {
    entries: Vec<RenameEntry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameEntry {
    target_symbol_id: String,
    expected_old_name: String,
    new_local_name: String,
}
fn rename_payload(value: &Value) -> Result<RenamePayload> {
    let p: RenamePayload =
        serde_json::from_value(value.clone()).map_err(|e| request_error(e.to_string()))?;
    if p.entries.is_empty() {
        return Err(error("Rename requires at least one entry"));
    }
    if p.entries.len() > 1000 {
        return Err(EditorError::new(
            "E-EDITOR-PLAN-LIMIT",
            "At most 1000 rename entries",
            413,
        ));
    }
    let mut ids = BTreeSet::new();
    for e in &p.entries {
        valid_name(&e.new_local_name)?;
        if !ids.insert(&e.target_symbol_id) {
            return Err(error("Duplicate rename target"));
        }
    }
    Ok(p)
}
pub(crate) fn validate_rename_payload(value: &Value) -> Result<()> {
    rename_payload(value).map(|_| ())
}
pub(crate) use detach::{plan_detach, validate_detach_payload};
#[derive(Default)]
struct Patches {
    files: BTreeMap<String, BTreeMap<(usize, usize), String>>,
}
impl Patches {
    fn add(
        &mut self,
        project: &ProjectSnapshot,
        span: &SourceSpan,
        replacement: String,
    ) -> Result<()> {
        let file = project
            .file_by_path(std::path::Path::new(&span.source))
            .ok_or_else(|| error("Unmapped source span"))?;
        if span.start_byte > span.end_byte
            || span.end_byte > file.text.len()
            || !file.text.is_char_boundary(span.start_byte)
            || !file.text.is_char_boundary(span.end_byte)
        {
            return Err(error("Invalid source span boundary"));
        }
        if file.text[span.start_byte..span.end_byte] == replacement {
            return Ok(());
        }
        let patches = self.files.entry(file.id.clone()).or_default();
        if let Some(old) = patches.get(&(span.start_byte, span.end_byte)) {
            if old != &replacement {
                return Err(error("Conflicting reference patches"));
            }
            return Ok(());
        }
        if patches
            .keys()
            .any(|&(start, end)| start < span.end_byte && span.start_byte < end)
        {
            return Err(error("Overlapping reference patches"));
        }
        patches.insert((span.start_byte, span.end_byte), replacement);
        Ok(())
    }
    fn apply(&self, project: &ProjectSnapshot) -> Result<Vec<(String, String)>> {
        let mut result = vec![];
        for (id, patches) in &self.files {
            let file = &project.files[id];
            if super::hash(file.text.as_bytes()) != file.hash {
                return Err(error("Source hash changed"));
            }
            let mut text = file.text.to_string();
            for (&(start, end), replacement) in patches.iter().rev() {
                text.replace_range(start..end, replacement);
            }
            // Rebuilding from untouched original slices proves patch-external bytes.
            let mut reconstructed = String::new();
            let mut cursor = 0;
            for (&(start, end), replacement) in patches {
                reconstructed.push_str(&file.text[cursor..start]);
                reconstructed.push_str(replacement);
                cursor = end;
            }
            reconstructed.push_str(&file.text[cursor..]);
            if reconstructed != text {
                return Err(error("Patch boundary preservation failed"));
            }
            result.push((id.clone(), text));
        }
        Ok(result)
    }
}
fn candidate(model: &EditorSession, changes: &EditChanges) -> Result<EditorSession> {
    let mut after = model.clone();
    if !after.commit_with_files(
        changes.text.clone(),
        changes.layouts.clone(),
        changes.added.clone(),
    )? {
        return Err(error("The request has no changes"));
    }
    after.project.parsed = parse_current_inputs(&after.project)?;
    after.parsed_hashes = after
        .project
        .ned_files()
        .map(|f| (f.id.clone(), f.hash.clone()))
        .collect();
    Ok(after)
}
pub(crate) fn plan_rename(model: &EditorSession, value: &Value) -> Result<EditChanges> {
    let payload = rename_payload(value)?;
    let before = inspect(&model.project)?;
    if model.project.layouts.values().any(|capture| {
        capture.adopted.is_none() && (capture.raw.is_some() || capture.warning.is_some())
    }) {
        return Err(error("Repair rejected layout before renaming symbols"));
    }
    let symbols = catalog(model)?;
    let mut names = Names::default();
    for e in payload.entries {
        let s = symbols
            .get(&e.target_symbol_id)
            .ok_or_else(|| error("Unknown or stale symbol ID"))?;
        if s.name != e.expected_old_name {
            return Err(error("Expected old symbol name changed"));
        }
        names.symbols.insert(s.clone(), e.new_local_name);
    }
    let mut final_symbols = BTreeSet::new();
    for s in symbols.values() {
        if !final_symbols.insert((
            names.qname(&s.owner),
            s.kind.clone(),
            names.name(&s.owner, &s.kind, &s.name),
        )) {
            return Err(error("Final symbol namespace collision"));
        }
    }
    let mut patches = Patches::default();
    for u in &before.uses {
        match u.syntax_kind {
            ReferenceSyntaxKind::IniParameterKey
            | ReferenceSyntaxKind::IniChannelId
            | ReferenceSyntaxKind::JsonStringToken => {
                let value = names.decoded(&u.decoded_value, &u.segments)?;
                if value != u.decoded_value {
                    let replacement = if u.syntax_kind == ReferenceSyntaxKind::JsonStringToken {
                        serde_json::to_string(&value).map_err(|e| error(e.to_string()))?
                    } else {
                        value
                    };
                    patches.add(&model.project, &u.span, replacement)?;
                }
            }
            ReferenceSyntaxKind::IniNetwork => {
                if let ReferenceTarget::Type { qname } = &u.target {
                    patches.add(&model.project, &u.span, names.qname(qname))?;
                }
            }
            ReferenceSyntaxKind::ChildType | ReferenceSyntaxKind::ChannelType => {
                if let Some(name) = names.target(&u.target) {
                    patches.add(
                        &model.project,
                        u.source_segments
                            .last()
                            .ok_or_else(|| error("Missing qualified name tokens"))?,
                        name,
                    )?;
                }
            }
            _ => {
                if let Some(name) = names.target(&u.target) {
                    patches.add(&model.project, &u.span, name)?;
                }
            }
        }
    }
    let mut changes = EditChanges {
        text: patches.apply(&model.project)?,
        ..EditChanges::default()
    };
    for (id, capture) in &model.project.layouts {
        let Some(layout) = &capture.adopted else {
            continue;
        };
        let mut updated = layout.clone();
        updated.types.clear();
        for (qname, entry) in &layout.types {
            let new_qname = names.qname(qname);
            let mut new_entry = entry.clone();
            new_entry.nodes.clear();
            for (node, position) in &entry.nodes {
                let key = names.name(qname, "child", node);
                if new_entry.nodes.insert(key, position.clone()).is_some() {
                    return Err(error(
                        "Final layout node key collision; repair stale entries first",
                    ));
                }
            }
            if updated.types.insert(new_qname, new_entry).is_some() {
                return Err(error(
                    "Final layout type key collision; repair stale entries first",
                ));
            }
        }
        if &updated != layout {
            changes.layouts.push((id.clone(), Some(updated)));
        }
    }
    let after_model = candidate(model, &changes)?;
    let after = inspect(&after_model.project)?;
    changes.semantic_evidence = semantic::verify_rename(
        &model.project,
        &after_model.project,
        &before,
        &after,
        &names,
    )?;
    for (old_id, s) in &symbols {
        changes.symbol_remap.push(json!({"before":symbol_dto(&model.project,old_id,s)?,"after":symbol_dto(&after_model.project,&symbol_id(&after_model.project,&names.qname(&s.owner),&s.kind,&names.name(&s.owner,&s.kind,&s.name))?,&Symbol{owner:names.qname(&s.owner),kind:s.kind.clone(),name:names.name(&s.owner,&s.kind,&s.name)})?}));
    }
    for i in &before.instances {
        let path = names.decoded(&i.path, &i.segments)?;
        changes
            .instance_remap
            .push(json!({"before":i.path,"after":path}));
    }
    Ok(changes)
}

fn symbol_dto(project: &ProjectSnapshot, id: &str, s: &Symbol) -> Result<Value> {
    for f in project.ned_files() {
        if let Some(parsed) = project.parsed.get(&f.id) {
            if let Some(ordinal) = parsed
                .declarations()
                .iter()
                .position(|d| d.name() == s.owner)
            {
                return Ok(
                    json!({"id":id,"owner":s.owner,"kind":s.kind,"name":s.name,"type_key":super::analysis::type_key(&f.id,ordinal,&s.owner)}),
                );
            }
        }
    }
    Err(error("Unmapped symbol DTO"))
}
