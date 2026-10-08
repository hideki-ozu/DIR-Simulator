//! Structural draft editing through source indexes and the common NED parser.
use super::analysis::{self, DeclarationShape, shapes};
use super::input::{CapturedFile, FileRole};
use super::layout::Position;
use super::model::EditorSession;
use super::source_index::{self, Patch, SourceIndex};
use super::{EditorError, Result, hash};
use crate::input::{identifier, reserved};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-COMPOSITION", message, 422)
}

fn text<'a>(payload: &'a Value, key: &str) -> Result<&'a str> {
    payload[key]
        .as_str()
        .ok_or_else(|| error(format!("Missing string {key}")))
}

fn name<'a>(payload: &'a Value, key: &str) -> Result<&'a str> {
    let value = text(payload, key)?;
    if !identifier(value) || reserved(value) {
        return Err(error(format!(
            "{key} must be a non-reserved NED identifier"
        )));
    }
    Ok(value)
}

fn position(payload: &Value) -> Result<Position> {
    let value = payload
        .get("position")
        .ok_or_else(|| error("Missing position"))?;
    let position: Position =
        serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    position.check()?;
    Ok(position)
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

fn qualified(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.into()
    } else {
        format!("{package}.{name}")
    }
}

fn bus(declaration: &DeclarationShape) -> bool {
    declaration.kind == "simple"
        && matches!(
            declaration.implementation.as_deref(),
            Some("dir.can.Bus" | "dir.can.MultibusBus")
        )
}

fn editable_gates(declaration: &DeclarationShape) -> Result<()> {
    if declaration.kind != "module" && !bus(declaration) {
        return Err(error(
            "Boundary gates can be edited only on compound modules or CAN Bus primitives",
        ));
    }
    Ok(())
}

fn bus_pairs(declaration: &DeclarationShape) -> Result<()> {
    if bus(declaration) {
        let outputs = declaration.gates.values().filter(|output| **output).count();
        let inputs = declaration.gates.len() - outputs;
        if inputs != outputs || inputs < 2 {
            return Err(error(
                "A Bus must retain balanced input/output gates and at least two pairs",
            ));
        }
    }
    Ok(())
}

fn unused_name(declaration: &DeclarationShape, name: &str) -> Result<()> {
    if declaration.gates.contains_key(name)
        || declaration.parameters.contains_key(name)
        || declaration.children.iter().any(|(child, _)| child == name)
    {
        return Err(error(format!(
            "Name {name} already exists in {}",
            declaration.name
        )));
    }
    Ok(())
}

/// Adopt only the explicitly expected structural changes, including all old declarations.
fn stage_checked(
    model: &mut EditorSession,
    id: &str,
    candidate: String,
    expected: &[DeclarationShape],
) -> Result<()> {
    let file = &model.project.files[id];
    let parsed = source_index::check_candidate(&candidate, &file.path, package(file), expected)?;
    let digest = hash(candidate.as_bytes());
    let file = model.project.files.get_mut(id).unwrap();
    file.hash = digest.clone();
    file.text = candidate.into();
    model.project.parsed.insert(id.into(), parsed);
    model.parsed_hashes.insert(id.into(), digest);
    model.syntax.insert(id.into(), "parsed".into());
    Ok(())
}

impl EditorSession {
    fn append_port_controller(&mut self, id: &str) -> Result<String> {
        let names: BTreeSet<_> = self
            .composition_sources()?
            .into_values()
            .flatten()
            .map(|d| d.name)
            .collect();
        let file = &self.project.files[id];
        let multibus = self.project.header.profile.as_deref() == Some("can.cc.multibus.v1");
        let (type_name, definition) =
            super::template::materialize("@builtin:Controller", package(file), multibus, &names)?;
        let fragment = format!("package {};\n{definition}", package(file));
        let generated = crate::input::parse_ned(&fragment, &file.path, package(file))
            .map_err(|d| EditorError::common("E-EDITOR-COMPOSITION", d))?;
        let mut expected = shapes(self.current_parse(id)?);
        expected.extend(shapes(&generated));
        let nl = source_index::newline(&file.text);
        let patch = Patch {
            span: file.text.len()..file.text.len(),
            replacement: format!("{nl}{nl}{}", definition.replace('\n', nl)),
        };
        let candidate = source_index::apply(&file.text, vec![patch])?;
        stage_checked(self, id, candidate, &expected)?;
        Ok(type_name)
    }

    /// Sources with pending or failed parsing cannot prove absence of references.
    fn composition_sources(&self) -> Result<BTreeMap<String, Vec<DeclarationShape>>> {
        self.project
            .ned_files()
            .map(|file| {
                self.current_parse(&file.id)
                    .map(|parsed| (file.id.clone(), shapes(parsed)))
                    .map_err(|e| {
                        error(format!(
                            "Cannot prove project references from {}: {}",
                            file.path.display(),
                            e.message
                        ))
                    })
            })
            .collect()
    }

    fn insert_gates(&mut self, payload: &Value) -> Result<()> {
        let (id, ordinal, mut declaration) = self.declaration(text(payload, "type_key")?)?;
        editable_gates(&declaration)?;
        let gates = payload["gates"]
            .as_array()
            .filter(|gates| !gates.is_empty())
            .ok_or_else(|| error("gates must be a nonempty array"))?;
        let mut statements = Vec::new();
        let mut added_outputs = 0;
        for gate in gates {
            let gate_name = name(gate, "name")?;
            unused_name(&declaration, gate_name)?;
            let output = gate["output"]
                .as_bool()
                .ok_or_else(|| error("Gate output must be a boolean"))?;
            added_outputs += usize::from(output);
            declaration.gates.insert(gate_name.into(), output);
            statements.push(format!(
                "{} {gate_name};",
                if output { "output" } else { "input" }
            ));
        }
        if bus(&declaration) && gates.len() - added_outputs != added_outputs {
            return Err(error(
                "Bus gates must be added in balanced input/output batches",
            ));
        }
        bus_pairs(&declaration)?;
        let file = &self.project.files[&id];
        let parsed = self.current_parse(&id)?;
        let mut expected = shapes(parsed);
        let indexes = SourceIndex::build(&file.text, parsed)?;
        let patch = source_index::insert_statement(
            &file.text,
            &indexes.declarations[ordinal],
            "gates",
            &statements.join(&format!("{}    ", source_index::newline(&file.text))),
        )?;
        expected[ordinal] = declaration;
        let candidate = source_index::apply(&file.text, vec![patch])?;
        stage_checked(self, &id, candidate, &expected)
    }

    fn remove_gate(
        &mut self,
        payload: &Value,
        sources: &BTreeMap<String, Vec<DeclarationShape>>,
    ) -> Result<()> {
        let (id, ordinal, mut declaration) = self.declaration(text(payload, "type_key")?)?;
        editable_gates(&declaration)?;
        let first = name(payload, "gate_name")?;
        let first_output = *declaration
            .gates
            .get(first)
            .ok_or_else(|| error("Unknown gate"))?;
        let mut removed = vec![first];
        if let Some(value) = payload.get("paired_gate").filter(|value| !value.is_null()) {
            let paired = value
                .as_str()
                .ok_or_else(|| error("paired_gate must be a string"))?;
            let paired_output = *declaration
                .gates
                .get(paired)
                .ok_or_else(|| error("Unknown paired gate"))?;
            if paired == first || paired_output == first_output {
                return Err(error(
                    "A gate pair requires distinct input and output gates",
                ));
            }
            removed.push(paired);
        } else if bus(&declaration) {
            return Err(error(
                "Deleting Bus gates requires paired_gate of the opposite direction",
            ));
        }
        if sources
            .values()
            .flatten()
            .filter(|d| d.name == declaration.name)
            .count()
            != 1
        {
            return Err(error(format!(
                "Type {} is ambiguous; gate references cannot be proven unused",
                declaration.name
            )));
        }
        let mut uses = Vec::new();
        for (file_id, declarations) in sources {
            for (parent_ordinal, parent) in declarations.iter().enumerate() {
                for (edge, connection) in parent.connections.iter().enumerate() {
                    let boundary_used = file_id == &id
                        && parent_ordinal == ordinal
                        && removed
                            .iter()
                            .any(|gate| connection.start == *gate || connection.end == *gate);
                    let child_used = parent.children.iter().any(|(child, ty)| {
                        ty == &declaration.name
                            && removed.iter().any(|gate| {
                                let endpoint = format!("{child}.{gate}");
                                connection.start == endpoint || connection.end == endpoint
                            })
                    });
                    if boundary_used || child_used {
                        uses.push(format!(
                            "{} declaration {} edge {} ({} --> {}) [{}]",
                            self.project.files[file_id].path.display(),
                            parent.name,
                            edge,
                            connection.start,
                            connection.end,
                            analysis::connection_key(
                                &analysis::type_key(file_id, parent_ordinal, &parent.name),
                                edge
                            )
                        ));
                    }
                }
            }
        }
        if !uses.is_empty() {
            return Err(error(format!(
                "Gate is used; disconnect explicitly before deletion: {}",
                uses.join("; ")
            )));
        }
        for gate in &removed {
            declaration.gates.remove(*gate);
        }
        bus_pairs(&declaration)?;
        let file = &self.project.files[&id];
        let parsed = self.current_parse(&id)?;
        let mut expected = shapes(parsed);
        let indexes = SourceIndex::build(&file.text, parsed)?;
        let section = indexes.declarations[ordinal]
            .sections
            .get("gates")
            .ok_or_else(|| error("Missing gate source index"))?;
        let patches = removed
            .iter()
            .map(|gate| {
                let statement = section
                    .statements
                    .iter()
                    .find(|s| s.tokens.get(1).is_some_and(|token| token.text == *gate))
                    .ok_or_else(|| error(format!("Gate {gate} has no source index")))?;
                source_index::delete_statement(&file.text, statement)
            })
            .collect::<Result<Vec<_>>>()?;
        expected[ordinal] = declaration;
        let candidate = source_index::apply(&file.text, patches)?;
        stage_checked(self, &id, candidate, &expected)
    }

    pub fn composition_command(&mut self, kind: &str, payload: &Value) -> Result<bool> {
        let sources = self.composition_sources()?;
        let mut staged = self.clone();
        match kind {
            "create_module" => {
                let parent_key = text(payload, "parent_type")?;
                let (id, _, parent) = staged.declaration(parent_key)?;
                if !matches!(parent.kind.as_str(), "module" | "network") {
                    return Err(error("A new module requires a compound parent"));
                }
                let short = name(payload, "type_name")?;
                let child = name(payload, "child_name")?;
                unused_name(&parent, child)?;
                let position = position(payload)?;
                let file = &staged.project.files[&id];
                let type_name = qualified(package(file), short);
                if sources.values().flatten().any(|d| d.name == type_name) {
                    return Err(error(format!("Type {type_name} already exists")));
                }
                let mut expected = shapes(staged.current_parse(&id)?);
                expected.push(DeclarationShape {
                    name: type_name.clone(),
                    kind: "module".into(),
                    implementation: None,
                    parameters: BTreeMap::new(),
                    gates: BTreeMap::new(),
                    children: vec![],
                    connections: vec![],
                });
                let nl = source_index::newline(&file.text);
                let patch = Patch {
                    span: file.text.len()..file.text.len(),
                    replacement: format!("{nl}{nl}module {short} {{{nl}}}{nl}"),
                };
                let candidate = source_index::apply(&file.text, vec![patch])?;
                stage_checked(&mut staged, &id, candidate, &expected)?;
                staged.graph_command(
                    "add_child",
                    &json!({
                        "parent_type":parent_key, "type_name":type_name,
                        "child_name":child, "position":position,
                    }),
                )?;
            }
            "add_gates" => staged.insert_gates(payload)?,
            "delete_gate" => staged.remove_gate(payload, &sources)?,
            "add_port" => {
                let key = text(payload, "type_key")?;
                let (id, _, declaration) = staged.declaration(key)?;
                if declaration.kind != "module" {
                    return Err(error("Ports require a compound module"));
                }
                let child = name(payload, "child_name")?;
                let input = name(payload, "input_gate")?;
                let output = name(payload, "output_gate")?;
                unused_name(&declaration, child)?;
                if child == input || child == output {
                    return Err(error("Port child and boundary gate names must be distinct"));
                }
                let position = position(payload)?;
                staged.insert_gates(&json!({"type_key":key,"gates":[
                    {"name":input,"output":false}, {"name":output,"output":true},
                ]}))?;
                let controller = staged.append_port_controller(&id)?;
                staged.graph_command(
                    "add_child",
                    &json!({
                        "parent_type":key, "type_name":controller,
                        "child_name":child, "position":position,
                    }),
                )?;
                for (from, to) in [
                    (input.to_owned(), format!("{child}.rx")),
                    (format!("{child}.tx"), output.to_owned()),
                ] {
                    staged.graph_command(
                        "connect",
                        &json!({"parent_type":key,"from":from,"to":to}),
                    )?;
                }
            }
            _ => return Err(error("Unknown composition command")),
        }
        let text = staged
            .project
            .files
            .values()
            .filter_map(|file| {
                self.project
                    .files
                    .get(&file.id)
                    .filter(|old| old.hash != file.hash)
                    .map(|_| (file.id.clone(), file.text.to_string()))
            })
            .collect();
        let layouts = staged
            .project
            .layouts
            .iter()
            .filter(|(id, layout)| {
                self.project
                    .layouts
                    .get(*id)
                    .is_some_and(|old| old.adopted != layout.adopted)
            })
            .map(|(id, layout)| (id.clone(), layout.adopted.clone()))
            .collect();
        let changed = self.commit_with_files(text, layouts, vec![])?;
        self.adopt_analysis(analysis::parse_project(&self.project), self.context_epoch);
        Ok(changed)
    }
}

#[cfg(test)]
#[path = "composition_tests.rs"]
mod tests;
