use super::super::input::{ned_package, relative_path, validate_added_file};
use super::super::layout::LayoutDocument;
use super::*;
use semantic::{CloneBinding, StopBinding};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachPayload {
    instance_id: String,
    expected_type_id: String,
    destination: Destination,
    clone_names: Vec<CloneName>,
    #[serde(default)]
    ancestor_names: Vec<AncestorName>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Destination {
    ExistingNed {
        file_id: String,
    },
    NewNed {
        root_id: String,
        relative_path: String,
        format: Format,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Format {
    newline: String,
    bom: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloneName {
    source_type_id: String,
    proposed_local_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AncestorName {
    owner_instance_id: String,
    proposed_local_name: String,
}
fn payload(value: &Value) -> Result<DetachPayload> {
    let p: DetachPayload =
        serde_json::from_value(value.clone()).map_err(|e| request_error(e.to_string()))?;
    if p.clone_names.len() > 64 || p.ancestor_names.len() > 64 {
        return Err(EditorError::new(
            "E-EDITOR-PLAN-LIMIT",
            "At most 64 clone/ancestor names",
            413,
        ));
    }
    let mut ids = BTreeSet::new();
    for n in &p.clone_names {
        valid_name(&n.proposed_local_name)?;
        if !ids.insert(&n.source_type_id) {
            return Err(error("Duplicate clone source"));
        }
    }
    let mut ids = BTreeSet::new();
    for n in &p.ancestor_names {
        valid_name(&n.proposed_local_name)?;
        if !ids.insert(&n.owner_instance_id) {
            return Err(error("Duplicate ancestor name"));
        }
    }
    if let Destination::NewNed {
        root_id,
        relative_path: path,
        format,
    } = &p.destination
    {
        let index = root_id
            .strip_prefix("ned-root-")
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| error("Invalid NED root ID"))?;
        if *root_id != format!("ned-root-{index}")
            || !matches!(format.newline.as_str(), "LF" | "CRLF")
        {
            return Err(error("Invalid destination format or root ID"));
        }
        let relative = relative_path(path, "ned")?;
        ned_package(&relative)?;
    }
    Ok(p)
}
pub(crate) fn validate_detach_payload(value: &Value) -> Result<()> {
    payload(value).map(|_| ())
}
fn qualify(
    project: &ProjectSnapshot,
    patches: &mut Patches,
    identifiers: &[SourceSpan],
    dots: &[SourceSpan],
    old: &str,
    new: &str,
) -> Result<()> {
    if identifiers.len() != old.split('.').count()
        || dots.len() + 1 != identifiers.len()
        || identifiers.len() < 2
    {
        return Err(error("Unsupported qualified token coverage"));
    }
    if package(old) == package(new) {
        return patches.add(project, identifiers.last().unwrap(), local(new).into());
    }
    patches.add(project, &identifiers[0], package(new).into())?;
    for span in &identifiers[1..identifiers.len() - 1] {
        patches.add(project, span, String::new())?;
    }
    for span in &dots[..dots.len() - 1] {
        patches.add(project, span, String::new())?;
    }
    patches.add(project, identifiers.last().unwrap(), local(new).into())
}
fn declaration<'a>(
    i: &'a ReferenceInspection,
    qname: &str,
) -> Result<&'a crate::input::Declaration> {
    i.declarations
        .iter()
        .find(|d| d.qname == qname)
        .map(|d| &d.declaration)
        .ok_or_else(|| error("Unknown declaration"))
}
fn copy_body(
    project: &ProjectSnapshot,
    d: &crate::input::Declaration,
    new: &str,
    rebind: &Option<(String, String)>,
) -> Result<String> {
    let file = project
        .file_by_path(std::path::Path::new(&d.span().source))
        .ok_or_else(|| error("Unknown clone source file"))?;
    let mut patches = Patches::default();
    patches.add(project, d.name_span(), local(new).into())?;
    if let Some((child, qname)) = rebind {
        let old = &d
            .children()
            .iter()
            .find(|(n, _)| n == child)
            .ok_or_else(|| error("Unknown clone child binding"))?
            .1;
        qualify(
            project,
            &mut patches,
            d.child_type_segments(child)
                .ok_or_else(|| error("Missing child name token evidence"))?,
            d.child_type_dots(child)
                .ok_or_else(|| error("Missing child dot evidence"))?,
            old,
            qname,
        )?;
    }
    let mut body = file.text[d.span().start_byte..d.span().end_byte].to_owned();
    for (&(start, end), replacement) in patches
        .files
        .get(&file.id)
        .into_iter()
        .flat_map(|p| p.iter().rev())
    {
        if start < d.span().start_byte || end > d.span().end_byte {
            return Err(error("Clone patch outside declaration"));
        }
        body.replace_range(
            start - d.span().start_byte..end - d.span().start_byte,
            replacement,
        );
    }
    Ok(body)
}
pub(crate) fn plan_detach(model: &EditorSession, value: &Value) -> Result<EditChanges> {
    let p = payload(value)?;
    let before = inspect(&model.project)?;
    let target = before
        .instances
        .iter()
        .find(|i| instance_id(&model.project, &i.path) == p.instance_id)
        .ok_or_else(|| error("Unknown or stale instance ID"))?;
    let d = declaration(&before, &target.type_qname)?;
    if !matches!(d.kind(), "simple" | "module") || target.parent_path.is_none() {
        return Err(error("Only simple/module child instances can be detached"));
    }
    if symbol_id(&model.project, d.name(), "type", local(d.name()))? != p.expected_type_id {
        return Err(error("Expected instance type changed"));
    }
    let (destination_id, destination_path, destination_package, new_role, newline, bom) =
        match &p.destination {
            Destination::ExistingNed { file_id } => {
                let f = model
                    .project
                    .files
                    .get(file_id)
                    .ok_or_else(|| error("Unknown destination file"))?;
                let package = f
                    .roles
                    .iter()
                    .find_map(|r| {
                        if let FileRole::Ned { package, .. } = r {
                            Some(package.clone())
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| error("Destination must be NED"))?;
                let newline = if f
                    .text
                    .find('\n')
                    .is_some_and(|n| n > 0 && f.text.as_bytes()[n - 1] == b'\r')
                {
                    "\r\n"
                } else {
                    "\n"
                };
                (
                    file_id.clone(),
                    f.path.clone(),
                    package,
                    None,
                    newline.to_owned(),
                    false,
                )
            }
            Destination::NewNed {
                root_id,
                relative_path: path,
                format,
            } => {
                let index = root_id
                    .strip_prefix("ned-root-")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let root = model
                    .project
                    .header
                    .roots
                    .get(index)
                    .ok_or_else(|| error("Unknown destination root"))?;
                let relative = relative_path(path, "ned")?;
                let package = ned_package(&relative)?;
                (
                    super::super::random_id("file-")?,
                    root.join(&relative),
                    package.clone(),
                    Some(FileRole::Ned {
                        root_index: index,
                        relative,
                        package,
                    }),
                    if format.newline == "CRLF" {
                        "\r\n"
                    } else {
                        "\n"
                    }
                    .into(),
                    format.bom,
                )
            }
        };
    let mut occupied: BTreeSet<_> = before
        .declarations
        .iter()
        .map(|d| d.qname.clone())
        .collect();
    let mut used_clone = BTreeSet::new();
    let mut used_ancestor = BTreeSet::new();
    let mut proposals = vec![];
    let mut ancestor_proposals = vec![];
    let mut clones = vec![];
    let mut current = target;
    let mut child_rebind = None;
    let stop = loop {
        if clones.len() >= 64 {
            return Err(EditorError::new(
                "E-EDITOR-PLAN-LIMIT",
                "Clone declaration/ancestor limit exceeded",
                413,
            ));
        }
        let source = declaration(&before, &current.type_qname)?;
        let source_id = symbol_id(&model.project, source.name(), "type", local(source.name()))?;
        let owner_id = instance_id(&model.project, &current.path);
        let explicit_clone = p.clone_names.iter().find(|n| n.source_type_id == source_id);
        let explicit_ancestor = p
            .ancestor_names
            .iter()
            .find(|n| n.owner_instance_id == owner_id);
        if explicit_ancestor.is_some() && current.path == target.path {
            return Err(error("Target name belongs in clone_names"));
        }
        if let (Some(a), Some(b)) = (explicit_clone, explicit_ancestor) {
            if a.proposed_local_name != b.proposed_local_name {
                return Err(error("Conflicting clone/ancestor name"));
            }
        }
        let name = if let Some(n) = explicit_clone {
            used_clone.insert(n.source_type_id.clone());
            n.proposed_local_name.clone()
        } else if let Some(n) = explicit_ancestor {
            n.proposed_local_name.clone()
        } else {
            let base = format!("{}For{}", local(source.name()), local(&current.path));
            let mut name = base.clone();
            let mut suffix = 2;
            while occupied.contains(&format!("{destination_package}.{name}")) {
                name = format!("{base}_{suffix}");
                suffix += 1;
            }
            name
        };
        if let Some(n) = explicit_ancestor {
            used_ancestor.insert(n.owner_instance_id.clone());
        }
        let qname = format!("{destination_package}.{name}");
        if !occupied.insert(qname.clone()) {
            return Err(error("Clone type name collision"));
        }
        proposals.push(json!({"source_type_id":source_id,"proposed_local_name":name}));
        if current.path != target.path {
            ancestor_proposals
                .push(json!({"owner_instance_id":owner_id,"proposed_local_name":name}));
        }
        clones.push(CloneBinding {
            source: current.type_qname.clone(),
            qname: qname.clone(),
            instance: current.path.clone(),
            child_rebind: child_rebind.take(),
        });
        let parent_path = current
            .parent_path
            .as_ref()
            .ok_or_else(|| error("Unexpected COW root"))?;
        let parent = before
            .instances
            .iter()
            .find(|i| i.path == *parent_path)
            .ok_or_else(|| error("Missing COW parent"))?;
        let owner = declaration(&before, &parent.type_qname)?;
        let child = local(&current.path).to_owned();
        let active_count = before
            .instances
            .iter()
            .filter(|i| i.type_qname == parent.type_qname)
            .count();
        let uses: Vec<_> = before
            .uses
            .iter()
            .filter(|u| {
                u.syntax_kind == ReferenceSyntaxKind::ChildType
                    && u.target
                        == ReferenceTarget::Type {
                            qname: parent.type_qname.clone(),
                        }
            })
            .collect();
        let incoming = if let Some(grandparent) = &parent.parent_path {
            let grandparent = before
                .instances
                .iter()
                .find(|i| i.path == *grandparent)
                .ok_or_else(|| error("Missing parent binding"))?;
            Some(
                declaration(&before, &grandparent.type_qname)?
                    .child_type_span(local(&parent.path))
                    .ok_or_else(|| error("Missing incoming parent type span"))?,
            )
        } else {
            None
        };
        let unique = active_count == 1
            && uses.len() == 1
            && incoming.is_some_and(|span| {
                uses[0].span.source == span.source
                    && uses[0].span.start_byte == span.start_byte
                    && uses[0].span.end_byte == span.end_byte
            });
        if owner.kind() == "network" || unique {
            break StopBinding {
                owner: owner.name().into(),
                child,
                qname,
                instance: parent.path.clone(),
            };
        }
        child_rebind = Some((child, qname));
        current = parent;
    };
    if used_clone.len() != p.clone_names.len() || used_ancestor.len() != p.ancestor_names.len() {
        return Err(error(
            "Name entry does not belong to the required clone chain",
        ));
    }
    let mut patches = Patches::default();
    let stopping = declaration(&before, &stop.owner)?;
    let old = &stopping
        .children()
        .iter()
        .find(|(n, _)| n == &stop.child)
        .ok_or_else(|| error("Unknown stopping child"))?
        .1;
    qualify(
        &model.project,
        &mut patches,
        stopping.child_type_segments(&stop.child).unwrap(),
        stopping.child_type_dots(&stop.child).unwrap(),
        old,
        &stop.qname,
    )?;
    let mut changes = EditChanges {
        text: patches.apply(&model.project)?,
        ..EditChanges::default()
    };
    let prefix = if new_role.is_some() {
        format!(
            "{}package {destination_package};{newline}",
            if bom { "\u{feff}" } else { "" }
        )
    } else {
        changes
            .text
            .iter()
            .find(|(id, _)| id == &destination_id)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| model.project.files[&destination_id].text.to_string())
    };
    let mut text = prefix;
    for clone in &clones {
        text.push_str(&newline);
        text.push_str(&newline);
        text.push_str(&copy_body(
            &model.project,
            declaration(&before, &clone.source)?,
            &clone.qname,
            &clone.child_rebind,
        )?);
        text.push_str(&newline);
    }
    if let Some(role) = new_role {
        let raw: Arc<str> = text.into();
        let file = CapturedFile {
            id: destination_id.clone(),
            path: destination_path.clone(),
            roles: vec![role],
            text: raw.clone(),
            hash: super::super::hash(raw.as_bytes()),
            origin_text: raw.clone(),
            origin_hash: super::super::hash(raw.as_bytes()),
            stat: None,
        };
        validate_added_file(&model.project, &file)?;
        changes.added.push(file);
    } else {
        changes.text.retain(|(id, _)| id != &destination_id);
        changes.text.push((destination_id.clone(), text));
    }
    for clone in &clones {
        let source = declaration(&before, &clone.source)?;
        let source_file = model
            .project
            .file_by_path(std::path::Path::new(&source.span().source))
            .unwrap();
        if model
            .project
            .layouts
            .get(&source_file.id)
            .is_some_and(|capture| {
                capture.adopted.is_none() && (capture.raw.is_some() || capture.warning.is_some())
            })
        {
            return Err(error("Repair rejected source layout before detaching"));
        }
    }
    if model
        .project
        .layouts
        .get(&destination_id)
        .is_some_and(|capture| {
            capture.adopted.is_none() && (capture.raw.is_some() || capture.warning.is_some())
        })
    {
        return Err(error("Repair rejected destination layout before detaching"));
    }
    let destination_layout = model
        .project
        .layouts
        .get(&destination_id)
        .and_then(|l| l.adopted.clone());
    let mut layout = destination_layout.unwrap_or_else(|| {
        LayoutDocument::empty(
            destination_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )
    });
    let mut layout_changed = false;
    for clone in &clones {
        let source = declaration(&before, &clone.source)?;
        let source_file = model
            .project
            .file_by_path(std::path::Path::new(&source.span().source))
            .unwrap();
        if let Some(source_layout) = model
            .project
            .layouts
            .get(&source_file.id)
            .and_then(|l| l.adopted.as_ref())
        {
            if let Some(entry) = source_layout.types.get(&clone.source) {
                if layout
                    .types
                    .insert(clone.qname.clone(), entry.clone())
                    .is_some()
                {
                    return Err(error("Destination layout type collision"));
                }
                layout_changed = true;
            }
        }
    }
    if layout_changed {
        changes.layouts.push((destination_id, Some(layout)));
    }
    let after_model = candidate(model, &changes)?;
    let after = inspect(&after_model.project)?;
    changes.semantic_evidence = semantic::verify_detach(
        &model.project,
        &after_model.project,
        &before,
        &after,
        &clones,
        &stop,
    )?;
    changes.semantic_evidence["clone_names"] = json!(proposals);
    changes.semantic_evidence["ancestor_names"] = json!(ancestor_proposals);
    changes.semantic_evidence["bindings"] = json!(
        clones
            .iter()
            .map(|c| json!({"instance":c.instance,"before_type":c.source,"after_type":c.qname}))
            .collect::<Vec<_>>()
    );
    changes.semantic_evidence["stopping_binding"] = json!({"owner_instance":stop.instance,"owner_type":stop.owner,"child":stop.child,"after_type":stop.qname,"active_count":before.instances.iter().filter(|i|i.type_qname==stop.owner).count(),"syntactic_uses":before.uses.iter().filter(|u|u.syntax_kind==ReferenceSyntaxKind::ChildType && u.target==ReferenceTarget::Type{qname:stop.owner.clone()}).count()});
    for (old_id, s) in catalog(model)? {
        let after_id = symbol_id(&after_model.project, &s.owner, &s.kind, &s.name)?;
        changes.symbol_remap.push(json!({"before":symbol_dto(&model.project,&old_id,&s)?,"after":symbol_dto(&after_model.project,&after_id,&s)?}));
    }
    for i in &before.instances {
        changes
            .instance_remap
            .push(json!({"before":i.path,"after":i.path}));
    }
    Ok(changes)
}
