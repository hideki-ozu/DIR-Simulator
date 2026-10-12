use super::analysis::{AnalysisBatch, DeclarationShape, shapes, type_key};
use super::input::{
    CapturedFile, FileRole, LayoutCapture, ProjectSnapshot, new_layout_capture,
    parse_current_inputs, validate_added_file,
};
use super::layout::LayoutDocument;
use super::{EditorError, Result, hash, random_id};
use crate::types::PreparedSimulation;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

const HISTORY_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug)]
pub(crate) struct TextPatch {
    pub file_id: String,
    pub expected_hash: String,
    pub span: Range<usize>,
    pub replacement: String,
}
#[derive(Clone, Debug)]
pub(crate) struct AddedFileBundle {
    pub file: CapturedFile,
    pub initial_layout_capture: Option<LayoutCapture>,
    pub generated_directories: Vec<PathBuf>,
}
#[derive(Clone, Debug)]
pub(crate) struct HistoryEntry {
    pub added: Vec<AddedFileBundle>,
    pub forward: Vec<TextPatch>,
    pub reverse: Vec<TextPatch>,
    pub layouts: Vec<(String, Option<LayoutDocument>, Option<LayoutDocument>)>,
    pub retained_bytes: usize,
}
#[derive(Clone)]
pub(crate) struct EditorSession {
    pub project_instance_id: String,
    pub project: ProjectSnapshot,
    pub revision: u64,
    pub input_revision: u64,
    pub context_epoch: u64,
    pub syntax: BTreeMap<String, String>,
    pub parsed_hashes: BTreeMap<String, String>,
    pub diagnostics: Vec<Value>,
    pub prepared: Option<Arc<PreparedSimulation>>,
    pub analysis: String,
    pub last_output: Option<String>,
    pub never_exported: bool,
    checkpoint: BTreeMap<String, String>,
    origin: BTreeMap<String, String>,
    history: Vec<HistoryEntry>,
    cursor: usize,
    retained_bytes: usize,
}
fn contents(project: &ProjectSnapshot) -> BTreeMap<String, String> {
    let mut result: BTreeMap<_, _> = project
        .files
        .values()
        .map(|f| (f.id.clone(), f.hash.clone()))
        .collect();
    for (id, l) in &project.layouts {
        if let Some(adopted) = &l.adopted {
            if let Ok(bytes) = adopted.bytes() {
                result.insert(format!("layout:{id}"), hash(&bytes));
            }
        }
    }
    result
}
fn patch_pair(id: &str, old: &str, new: &str) -> (TextPatch, TextPatch) {
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let mut suffix = old[start..]
        .bytes()
        .rev()
        .zip(new[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (
        TextPatch {
            file_id: id.into(),
            expected_hash: hash(old.as_bytes()),
            span: start..old.len() - suffix,
            replacement: new[start..new.len() - suffix].into(),
        },
        TextPatch {
            file_id: id.into(),
            expected_hash: hash(new.as_bytes()),
            span: start..new.len() - suffix,
            replacement: old[start..old.len() - suffix].into(),
        },
    )
}
impl EditorSession {
    pub fn new(project: ProjectSnapshot) -> Self {
        debug_assert!(
            project
                .files
                .values()
                .all(|f| hash(f.origin_text.as_bytes()) == f.origin_hash)
        );
        let checkpoint = contents(&project);
        let parsed_hashes = project
            .ned_files()
            .map(|f| (f.id.clone(), f.hash.clone()))
            .collect();
        let syntax = project
            .ned_files()
            .map(|f| (f.id.clone(), "parsed".into()))
            .collect();
        Self {
            project_instance_id: project.id.clone(),
            project,
            revision: 1,
            input_revision: 1,
            context_epoch: 1,
            syntax,
            parsed_hashes,
            diagnostics: vec![],
            prepared: None,
            analysis: "unchecked".into(),
            last_output: None,
            never_exported: true,
            origin: checkpoint.clone(),
            checkpoint,
            history: vec![],
            cursor: 0,
            retained_bytes: 0,
        }
    }
    pub fn dirty(&self) -> bool {
        contents(&self.project) != self.checkpoint
    }
    pub fn file_dirty(&self, id: &str) -> bool {
        self.project
            .files
            .get(id)
            .is_some_and(|f| self.checkpoint.get(id) != Some(&f.hash))
            || self
                .project
                .layouts
                .get(id)
                .and_then(|l| l.adopted.as_ref())
                .is_some_and(|l| {
                    l.bytes().is_ok_and(|bytes| {
                        self.checkpoint.get(&format!("layout:{id}")) != Some(&hash(&bytes))
                    })
                })
    }
    pub fn differs_from_origin(&self) -> bool {
        contents(&self.project) != self.origin
    }
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }
    pub fn can_redo(&self) -> bool {
        self.cursor < self.history.len()
    }
    pub fn check_revision(&self, revision: u64) -> Result<()> {
        if revision != self.revision {
            Err(EditorError::new(
                "E-EDITOR-REVISION",
                "The model changed; refresh before applying this operation",
                409,
            ))
        } else {
            Ok(())
        }
    }
    pub fn current_parse(&self, id: &str) -> Result<&crate::input::ParsedNed> {
        let f = self
            .project
            .files
            .get(id)
            .ok_or_else(|| EditorError::new("E-EDITOR-UNMAPPED", "Unknown file", 422))?;
        if self.parsed_hashes.get(id) != Some(&f.hash) {
            return Err(EditorError::new(
                "E-EDITOR-UNMAPPED",
                "Current source has no usable structure index",
                422,
            ));
        }
        self.project
            .parsed
            .get(id)
            .ok_or_else(|| EditorError::new("E-EDITOR-UNMAPPED", "No parsed file", 422))
    }
    pub fn declaration(&self, key: &str) -> Result<(String, usize, DeclarationShape)> {
        for f in self.project.ned_files() {
            if let Ok(parsed) = self.current_parse(&f.id) {
                for (ordinal, d) in shapes(parsed).into_iter().enumerate() {
                    if type_key(&f.id, ordinal, &d.name) == key {
                        return Ok((f.id.clone(), ordinal, d));
                    }
                }
            }
        }
        Err(EditorError::new(
            "E-EDITOR-UNMAPPED",
            "Selected type is unavailable or stale",
            422,
        ))
    }
    pub fn unique_type(&self, name: &str) -> Result<DeclarationShape> {
        let mut matches = Vec::new();
        for f in self.project.ned_files() {
            if let Ok(parsed) = self.current_parse(&f.id) {
                matches.extend(shapes(parsed).into_iter().filter(|d| d.name == name));
            }
        }
        if matches.len() != 1 {
            return Err(EditorError::new(
                "E-EDITOR-UNMAPPED",
                "Type reference is unresolved or ambiguous",
                422,
            ));
        }
        Ok(matches.remove(0))
    }
    pub fn commit(
        &mut self,
        text: Vec<(String, String)>,
        layout: Vec<(String, Option<LayoutDocument>)>,
    ) -> Result<bool> {
        self.commit_with_files(text, layout, vec![])
    }
    pub fn commit_with_files(
        &mut self,
        text: Vec<(String, String)>,
        layout: Vec<(String, Option<LayoutDocument>)>,
        added: Vec<CapturedFile>,
    ) -> Result<bool> {
        let mut entry = HistoryEntry {
            added: vec![],
            forward: vec![],
            reverse: vec![],
            layouts: vec![],
            retained_bytes: 0,
        };
        if !added.is_empty() {
            parse_current_inputs(&self.project)?;
        }
        let mut candidate = self.project.clone();
        for file in added {
            validate_added_file(&candidate, &file)?;
            let initial_layout_capture = file
                .roles
                .iter()
                .any(|r| matches!(r, FileRole::Ned { .. }))
                .then(|| new_layout_capture(&file));
            entry.retained_bytes = entry
                .retained_bytes
                .checked_add(file.text.len())
                .ok_or_else(|| {
                    EditorError::new("E-EDITOR-HISTORY-LIMIT", "History size overflow", 413)
                })?;
            if let Some(capture) = &initial_layout_capture {
                candidate.layouts.insert(file.id.clone(), capture.clone());
            }
            candidate.files.insert(file.id.clone(), file.clone());
            let before = candidate
                .directories
                .keys()
                .chain(candidate.metadata.keys())
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            candidate.refresh_inputs();
            let generated_directories = candidate
                .directories
                .keys()
                .filter(|p| !before.contains(*p))
                .cloned()
                .collect();
            entry.added.push(AddedFileBundle {
                file,
                initial_layout_capture,
                generated_directories,
            });
        }
        for (id, new) in text {
            let old = self
                .project
                .files
                .get(&id)
                .ok_or_else(|| EditorError::new("E-EDITOR-PATCH", "Unknown file", 422))?;
            if entry.forward.iter().any(|p| p.file_id == id) {
                return Err(EditorError::new(
                    "E-EDITOR-PATCH",
                    "Duplicate text target",
                    422,
                ));
            }
            let candidate_file = candidate.files.get_mut(&id).unwrap();
            candidate_file.hash = hash(new.as_bytes());
            candidate_file.text = new.clone().into();
            if old.text.as_ref() != new {
                let (forward, reverse) = patch_pair(&id, &old.text, &new);
                entry.retained_bytes = entry
                    .retained_bytes
                    .checked_add(forward.replacement.len() + reverse.replacement.len())
                    .ok_or_else(|| {
                        EditorError::new("E-EDITOR-HISTORY-LIMIT", "History size overflow", 413)
                    })?;
                entry.forward.push(forward);
                entry.reverse.push(reverse);
            }
        }
        for (id, new) in layout {
            let old = candidate
                .layouts
                .get(&id)
                .ok_or_else(|| EditorError::new("E-EDITOR-LAYOUT", "Unknown layout", 422))?
                .adopted
                .clone();
            if entry.layouts.iter().any(|(target, _, _)| target == &id) {
                return Err(EditorError::new(
                    "E-EDITOR-LAYOUT",
                    "Duplicate layout target",
                    422,
                ));
            }
            if old != new {
                let bytes = serde_json::to_vec(&(&old, &new))
                    .map_err(|e| EditorError::new("E-EDITOR-LAYOUT", e.to_string(), 422))?
                    .len();
                entry.retained_bytes =
                    entry.retained_bytes.checked_add(bytes).ok_or_else(|| {
                        EditorError::new("E-EDITOR-HISTORY-LIMIT", "History size overflow", 413)
                    })?;
                entry.layouts.push((id, old, new));
            }
        }
        if entry.forward.is_empty() && entry.layouts.is_empty() && entry.added.is_empty() {
            return Ok(false);
        }
        if entry.retained_bytes > HISTORY_BYTES {
            return Err(EditorError::new(
                "E-EDITOR-HISTORY-LIMIT",
                "One edit exceeds the 32 MiB undo limit",
                413,
            ));
        }
        if !entry.added.is_empty() {
            let config = candidate.file_by_path(&candidate.config).unwrap();
            let header =
                crate::input::inspect_config(&config.text, &candidate.config, &candidate.cwd)
                    .map_err(|d| EditorError::common("E-EDITOR-CREATE-FILE", d))?;
            if header.roots != self.project.header.roots {
                return Err(EditorError::new(
                    "E-EDITOR-CREATE-FILE",
                    "An addition cannot change NED roots",
                    422,
                ));
            }
            for bundle in &entry.added {
                let reference = match bundle.file.roles[0] {
                    FileRole::Workload => header.workload.as_ref(),
                    FileRole::ModelConfig => header.model_config.as_ref(),
                    _ => continue,
                };
                if reference != Some(&bundle.file.path) {
                    return Err(EditorError::new(
                        "E-EDITOR-CREATE-FILE",
                        "Added JSON must have its matching INI reference",
                        422,
                    ));
                }
            }
            parse_current_inputs(&candidate)?;
        }
        self.apply_entry(&entry, false)?;
        for removed in self.history.drain(self.cursor..) {
            self.retained_bytes -= removed.retained_bytes;
        }
        self.retained_bytes += entry.retained_bytes;
        self.history.push(entry);
        self.cursor = self.history.len();
        while self.history.len() > 200 || self.retained_bytes > HISTORY_BYTES {
            let removed = self.history.remove(0);
            self.retained_bytes -= removed.retained_bytes;
            self.cursor -= 1;
        }
        Ok(true)
    }
    fn apply_entry(&mut self, entry: &HistoryEntry, reverse: bool) -> Result<()> {
        let patches = if reverse {
            &entry.reverse
        } else {
            &entry.forward
        };
        let mut project = self.project.clone();
        if !reverse {
            for bundle in &entry.added {
                let file = &bundle.file;
                if project.files.contains_key(&file.id)
                    || project.file_by_path(&file.path).is_some()
                {
                    return Err(EditorError::new(
                        "E-EDITOR-PATCH",
                        "Added file already exists",
                        409,
                    ));
                }
                if let Some(capture) = &bundle.initial_layout_capture {
                    if project.layouts.contains_key(&file.id) {
                        return Err(EditorError::new(
                            "E-EDITOR-PATCH",
                            "Added layout already exists",
                            409,
                        ));
                    }
                    project.layouts.insert(file.id.clone(), capture.clone());
                }
                project.files.insert(file.id.clone(), file.clone());
            }
        }
        for patch in patches {
            let f = project
                .files
                .get_mut(&patch.file_id)
                .ok_or_else(|| EditorError::new("E-EDITOR-PATCH", "Unknown history file", 422))?;
            if f.hash != patch.expected_hash
                || patch.span.start > patch.span.end
                || patch.span.end > f.text.len()
                || !f.text.is_char_boundary(patch.span.start)
                || !f.text.is_char_boundary(patch.span.end)
            {
                return Err(EditorError::new(
                    "E-EDITOR-PATCH",
                    "History source precondition failed",
                    409,
                ));
            }
            let mut text = f.text.to_string();
            text.replace_range(patch.span.clone(), &patch.replacement);
            f.hash = hash(text.as_bytes());
            f.text = text.into();
        }
        for (id, before, after) in &entry.layouts {
            let l = project.layouts.get_mut(id).ok_or_else(|| {
                EditorError::new("E-EDITOR-LAYOUT", "Unknown history layout", 422)
            })?;
            let (expected, replacement) = if reverse {
                (after, before)
            } else {
                (before, after)
            };
            if &l.adopted != expected {
                return Err(EditorError::new(
                    "E-EDITOR-PATCH",
                    "History layout precondition failed",
                    409,
                ));
            }
            l.adopted = replacement.clone();
        }
        if reverse {
            for bundle in entry.added.iter().rev() {
                let file = &bundle.file;
                if project
                    .files
                    .get(&file.id)
                    .is_none_or(|current| current.hash != file.hash)
                {
                    return Err(EditorError::new(
                        "E-EDITOR-PATCH",
                        "Added file history precondition failed",
                        409,
                    ));
                }
                project.files.remove(&file.id);
                project.parsed.remove(&file.id);
                project.layouts.remove(&file.id);
                project.metadata.remove(&file.path);
                if let (Some(parent), Some(name)) = (file.path.parent(), file.path.file_name()) {
                    if let Some(entries) = project.directories.get_mut(parent) {
                        entries.retain(|(n, _)| n != name);
                    }
                }
                for dir in bundle.generated_directories.iter().rev() {
                    if project
                        .directories
                        .get(dir)
                        .is_some_and(|entries| entries.is_empty())
                        && !project.files.values().any(|f| f.path.starts_with(dir))
                        && !project.layouts.values().any(|l| l.path.starts_with(dir))
                    {
                        project.directories.remove(dir);
                        project.metadata.remove(dir);
                        if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
                            if let Some(entries) = project.directories.get_mut(parent) {
                                entries.retain(|(n, _)| n != name);
                            }
                        }
                    }
                }
            }
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| EditorError::new("E-EDITOR-REVISION", "Revision overflow", 409))?;
        let input_changed = !patches.is_empty() || !entry.added.is_empty();
        let input_revision = self
            .input_revision
            .checked_add(u64::from(input_changed))
            .ok_or_else(|| EditorError::new("E-EDITOR-REVISION", "Input revision overflow", 409))?;
        if input_changed {
            project.id = random_id("snapshot-")?;
        }
        if !reverse {
            for bundle in &entry.added {
                if let FileRole::Ned { package, .. } = &bundle.file.roles[0] {
                    let file = &bundle.file;
                    let parsed = crate::input::parse_ned(&file.text, &file.path, package)
                        .map_err(|d| EditorError::common("E-EDITOR-CREATE-FILE", d))?;
                    project.parsed.insert(file.id.clone(), parsed);
                }
            }
        }
        project.refresh_inputs();
        self.project = project;
        self.revision = revision;
        self.input_revision = input_revision;
        for bundle in &entry.added {
            let id = &bundle.file.id;
            if reverse {
                self.syntax.remove(id);
                self.parsed_hashes.remove(id);
                self.diagnostics.retain(|d| d["file_id"] != *id);
            } else if bundle.initial_layout_capture.is_some() {
                self.syntax.insert(id.clone(), "pending".into());
                self.parsed_hashes.remove(id);
            }
        }
        if !patches.is_empty() || !entry.added.is_empty() {
            self.prepared = None;
            self.analysis = "pending".into();
            for p in patches {
                if self
                    .project
                    .files
                    .get(&p.file_id)
                    .is_some_and(|f| f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })))
                {
                    self.syntax.insert(p.file_id.clone(), "pending".into());
                    self.parsed_hashes.remove(&p.file_id);
                }
            }
            self.diagnostics.retain(|d| d["origin"] == "layout");
        }
        Ok(())
    }
    pub fn undo(&mut self) -> Result<bool> {
        if self.cursor == 0 {
            return Ok(false);
        }
        let entry = self.history[self.cursor - 1].clone();
        self.apply_entry(&entry, true)?;
        self.cursor -= 1;
        Ok(true)
    }
    pub fn redo(&mut self) -> Result<bool> {
        if self.cursor == self.history.len() {
            return Ok(false);
        }
        let entry = self.history[self.cursor].clone();
        self.apply_entry(&entry, false)?;
        self.cursor += 1;
        Ok(true)
    }
    pub fn adopt_analysis(&mut self, batch: AnalysisBatch, epoch: u64) {
        if epoch != self.context_epoch {
            return;
        }
        for (id, file) in batch.files {
            if self
                .project
                .files
                .get(&id)
                .is_none_or(|f| f.hash != file.hash)
            {
                continue;
            }
            self.diagnostics
                .retain(|d| d["origin"] != "syntax" || d["file_id"] != id);
            match file.result {
                Ok(parsed) => {
                    self.project.parsed.insert(id.clone(), parsed);
                    self.parsed_hashes.insert(id.clone(), file.hash);
                    self.syntax.insert(id, "parsed".into());
                }
                Err(d) => {
                    self.parsed_hashes.remove(&id);
                    self.syntax.insert(id.clone(), "syntax_error".into());
                    self.diagnostics.push(json!({"origin":"syntax","code":d.code,"severity":"error","message":d.message,"file_id":id,"input_revision":self.input_revision.to_string(),"snapshot_id":batch.snapshot_id}));
                }
            }
        }
        if self.syntax.values().any(|s| s == "syntax_error") {
            self.analysis = "syntax_error".into();
        } else if self.syntax.values().any(|s| s == "pending") {
            self.analysis = "pending".into();
        } else if self.prepared.is_none() {
            self.analysis = "unchecked".into();
        }
    }
    pub fn mark_saved(&mut self, path: String) {
        self.checkpoint = contents(&self.project);
        self.last_output = Some(path);
        self.never_exported = false;
    }
    pub fn reload(&mut self, project: ProjectSnapshot) -> Result<()> {
        let project_instance_id = random_id("project-")?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| EditorError::new("E-EDITOR-REVISION", "Revision overflow", 409))?;
        let changed = project.input_digest() != self.project.input_digest();
        let input_revision = self
            .input_revision
            .checked_add(u64::from(changed))
            .ok_or_else(|| EditorError::new("E-EDITOR-REVISION", "Input revision overflow", 409))?;
        let epoch = self.context_epoch.checked_add(1).ok_or_else(|| {
            EditorError::new("E-EDITOR-REVISION", "Context revision overflow", 409)
        })?;
        let last_output = self.last_output.clone();
        let never_exported = self.never_exported;
        *self = Self::new(project);
        self.project_instance_id = project_instance_id;
        self.revision = revision;
        self.input_revision = input_revision;
        self.context_epoch = epoch;
        self.last_output = last_output;
        self.never_exported = never_exported;
        Ok(())
    }
}
