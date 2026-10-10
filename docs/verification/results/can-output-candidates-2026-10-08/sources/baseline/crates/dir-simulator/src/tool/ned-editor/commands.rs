use super::analysis::{ConnectionShape, DeclarationShape};
use super::input::FileRole;
use super::layout::{LayoutDocument, Position};
use super::model::EditorSession;
use super::source_index::{self, SourceIndex};
use super::{EditorError, Result};
use serde_json::Value;

fn error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-UNMAPPED", message, 422)
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error(format!("Missing string {key}")))
}
fn channel(v: &Value, key: &str) -> Result<Option<String>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(error(format!("{key} must be a string or null"))),
    }
}
fn contained_literal(source: &str) -> Result<()> {
    let tokens = source_index::tokens(source)?;
    let mut end = 0;
    for token in &tokens {
        if !source[end..token.span.start]
            .chars()
            .all(char::is_whitespace)
            || (!token.text.starts_with('"')
                && !token
                    .text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')))
        {
            return Err(error(
                "Default value must contain only a literal, without statements or comments",
            ));
        }
        end = token.span.end;
    }
    if tokens.is_empty() || !source[end..].chars().all(char::is_whitespace) {
        return Err(error("Default value must contain only a literal"));
    }
    Ok(())
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .enumerate()
            .all(|(i, c)| c.is_ascii_alphabetic() || c == b'_' || (i > 0 && c.is_ascii_digit()))
        && !matches!(
            s,
            "parameters"
                | "gates"
                | "submodules"
                | "connections"
                | "simple"
                | "module"
                | "network"
                | "channel"
                | "input"
                | "output"
                | "package"
        )
}
fn connection_text(c: &ConnectionShape) -> String {
    match &c.channel {
        Some(ch) => format!("{} --> {ch} --> {};", c.start, c.end),
        None => format!("{} --> {};", c.start, c.end),
    }
}
fn token_span(tokens: &[source_index::Token]) -> Result<std::ops::Range<usize>> {
    let first = tokens
        .first()
        .ok_or_else(|| error("Missing indexed tokens"))?;
    let last = tokens
        .last()
        .ok_or_else(|| error("Missing indexed tokens"))?;
    Ok(first.span.start..last.span.end)
}
impl EditorSession {
    fn endpoint(&self, d: &DeclarationShape, e: &str, source: bool) -> Result<()> {
        let output = if let Some((child, gate)) = e.split_once('.') {
            let ty = d
                .children
                .iter()
                .find(|(name, _)| name == child)
                .ok_or_else(|| error("Unknown child endpoint"))?;
            *self
                .unique_type(&ty.1)?
                .gates
                .get(gate)
                .ok_or_else(|| error("Unknown child gate"))?
        } else {
            !*d.gates
                .get(e)
                .ok_or_else(|| error("Unknown boundary gate"))?
        };
        if output != source {
            return Err(error("Gate direction is incompatible with this endpoint"));
        }
        Ok(())
    }
    fn check_connection(
        &self,
        d: &DeclarationShape,
        c: &ConnectionShape,
        except: Option<usize>,
    ) -> Result<()> {
        self.endpoint(d, &c.start, true)?;
        self.endpoint(d, &c.end, false)?;
        if d.connections
            .iter()
            .enumerate()
            .any(|(i, other)| Some(i) != except && (other.start == c.start || other.end == c.end))
        {
            return Err(error("Gate is already connected"));
        }
        if let Some(channel) = &c.channel {
            if self.unique_type(channel)?.kind != "channel" {
                return Err(error("Selected connection type is not a channel"));
            }
        }
        Ok(())
    }
    pub fn graph_command(&mut self, kind: &str, p: &Value) -> Result<bool> {
        if matches!(
            kind,
            "create_module" | "add_gates" | "delete_gate" | "add_port"
        ) {
            return self.composition_command(kind, p);
        }
        if ["type_name", "channel", "tx_channel", "rx_channel"]
            .iter()
            .any(|key| p[*key].as_str().is_some_and(|s| s.starts_with("@builtin:")))
        {
            return self.builtin_command(kind, p);
        }
        let (key, selected) = match kind {
            "delete_child" => text(p, "element_key")?
                .rsplit_once("::node::")
                .map(|(a, b)| (a.to_owned(), Some(b.to_owned())))
                .ok_or_else(|| error("Invalid node key"))?,
            "disconnect" | "reconnect" => text(p, "connection_key")?
                .rsplit_once("::connection::")
                .map(|(a, b)| (a.to_owned(), Some(b.to_owned())))
                .ok_or_else(|| error("Invalid connection key"))?,
            "set_default" => text(p, "parameter_key")?
                .rsplit_once("::parameter::")
                .map(|(a, b)| (a.to_owned(), Some(b.to_owned())))
                .ok_or_else(|| error("Invalid parameter key"))?,
            "set_layout" => (text(p, "type_key")?.into(), None),
            _ => (text(p, "parent_type")?.into(), None),
        };
        let (file_id, ordinal, mut d) = self.declaration(&key)?;
        let file = self
            .project
            .files
            .get(&file_id)
            .ok_or_else(|| error("Unknown file"))?
            .clone();
        let mut layout = self
            .project
            .layouts
            .get(&file_id)
            .and_then(|l| l.adopted.clone());
        if kind == "set_layout" {
            let positions = p
                .get("positions")
                .and_then(Value::as_object)
                .ok_or_else(|| error("Missing positions"))?;
            let doc = layout.get_or_insert_with(|| {
                LayoutDocument::empty(file.path.file_name().unwrap().to_string_lossy().into())
            });
            let ty = doc.types.entry(d.name.clone()).or_default();
            for (name, value) in positions {
                if !d.children.iter().any(|(n, _)| n == name) {
                    return Err(error("Unknown layout node"));
                }
                let position: Position =
                    serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
                position.check()?;
                ty.nodes.insert(name.clone(), position);
            }
            return self.commit(vec![], vec![(file_id, layout)]);
        }
        let parsed = self.current_parse(&file_id)?;
        let mut expected = super::analysis::shapes(parsed);
        let indexes = SourceIndex::build(&file.text, parsed)?;
        let index = &indexes.declarations[ordinal];
        let mut patches = Vec::new();
        match kind {
            "add_child" => {
                if !matches!(d.kind.as_str(), "module" | "network") {
                    return Err(error("Children require a compound type"));
                }
                let name = text(p, "child_name")?;
                let ty = text(p, "type_name")?;
                if !identifier(name) || d.children.iter().any(|(n, _)| n == name) {
                    return Err(error("Child name is invalid or already present"));
                }
                if !matches!(self.unique_type(ty)?.kind.as_str(), "simple" | "module") {
                    return Err(error("Child must be a simple or compound module"));
                }
                patches.push(source_index::insert_statement(
                    &file.text,
                    index,
                    "submodules",
                    &format!("{name}: {ty};"),
                )?);
                d.children.push((name.into(), ty.into()));
                if let Some(value) = p.get("position") {
                    let position: Position =
                        serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
                    position.check()?;
                    layout
                        .get_or_insert_with(|| {
                            LayoutDocument::empty(
                                file.path.file_name().unwrap().to_string_lossy().into(),
                            )
                        })
                        .types
                        .entry(d.name.clone())
                        .or_default()
                        .nodes
                        .insert(name.into(), position);
                }
            }
            "delete_child" => {
                let name = selected.as_deref().unwrap();
                let child = d
                    .children
                    .iter()
                    .position(|(n, _)| n == name)
                    .ok_or_else(|| error("Unknown child"))?;
                patches.push(source_index::delete_statement(
                    &file.text,
                    &index
                        .sections
                        .get("submodules")
                        .ok_or_else(|| error("Missing submodules index"))?
                        .statements[child],
                )?);
                d.children.remove(child);
                let prefix = format!("{name}.");
                let mut removed = Vec::new();
                for (i, c) in d.connections.iter().enumerate() {
                    if c.start.starts_with(&prefix) || c.end.starts_with(&prefix) {
                        patches.push(source_index::delete_statement(
                            &file.text,
                            &index.sections["connections"].statements[i],
                        )?);
                        removed.push(i);
                    }
                }
                for i in removed.into_iter().rev() {
                    d.connections.remove(i);
                }
                if let Some(doc) = layout.as_mut() {
                    if let Some(ty) = doc.types.get_mut(&d.name) {
                        ty.nodes.remove(name);
                    }
                }
            }
            "connect" => {
                let c = ConnectionShape {
                    start: text(p, "from")?.into(),
                    end: text(p, "to")?.into(),
                    channel: channel(p, "channel")?,
                };
                self.check_connection(&d, &c, None)?;
                patches.push(source_index::insert_statement(
                    &file.text,
                    index,
                    "connections",
                    &connection_text(&c),
                )?);
                d.connections.push(c);
            }
            "connect_can_pair" => {
                let controller = text(p, "controller")?;
                let bus = text(p, "bus")?;
                let ctrl_ty = d
                    .children
                    .iter()
                    .find(|(n, _)| n == controller)
                    .ok_or_else(|| error("Unknown controller"))?;
                let bus_ty = d
                    .children
                    .iter()
                    .find(|(n, _)| n == bus)
                    .ok_or_else(|| error("Unknown bus"))?;
                let ctrl = self.unique_type(&ctrl_ty.1)?;
                let bus_shape = self.unique_type(&bus_ty.1)?;
                if !matches!(
                    ctrl.implementation.as_deref(),
                    Some("dir.can.Controller" | "dir.can.MultibusController")
                ) || !matches!(
                    bus_shape.implementation.as_deref(),
                    Some("dir.can.Bus" | "dir.can.MultibusBus")
                ) {
                    return Err(error(
                        "CAN pair requires Controller and Bus implementations",
                    ));
                }
                let outs: Vec<_> = ctrl
                    .gates
                    .iter()
                    .filter(|(_, o)| **o)
                    .map(|(n, _)| n)
                    .collect();
                let ins: Vec<_> = ctrl
                    .gates
                    .iter()
                    .filter(|(_, o)| !**o)
                    .map(|(n, _)| n)
                    .collect();
                if outs.len() != 1 || ins.len() != 1 {
                    return Err(error("Controller gates are ambiguous"));
                }
                let a = ConnectionShape {
                    start: format!("{controller}.{}", outs[0]),
                    end: format!("{bus}.{}", text(p, "input_gate")?),
                    channel: channel(p, "tx_channel")?,
                };
                let b = ConnectionShape {
                    start: format!("{bus}.{}", text(p, "output_gate")?),
                    end: format!("{controller}.{}", ins[0]),
                    channel: channel(p, "rx_channel")?,
                };
                self.check_connection(&d, &a, None)?;
                d.connections.push(a.clone());
                self.check_connection(&d, &b, None)?;
                d.connections.push(b.clone());
                patches.push(source_index::insert_statement(
                    &file.text,
                    index,
                    "connections",
                    &format!(
                        "{}{}    {}",
                        connection_text(&a),
                        source_index::newline(&file.text),
                        connection_text(&b)
                    ),
                )?);
            }
            "disconnect" | "reconnect" => {
                let i: usize = selected
                    .as_deref()
                    .unwrap()
                    .parse()
                    .map_err(|_| error("Invalid connection ordinal"))?;
                let old = d
                    .connections
                    .get(i)
                    .ok_or_else(|| error("Unknown connection"))?
                    .clone();
                let statement = &index.sections["connections"].statements[i];
                if kind == "disconnect" {
                    patches.push(source_index::delete_statement(&file.text, statement)?);
                    d.connections.remove(i);
                } else {
                    let mut c = old.clone();
                    let side = text(p, "endpoint_side")?;
                    match side {
                        "start" => c.start = text(p, "new_endpoint")?.into(),
                        "end" => c.end = text(p, "new_endpoint")?.into(),
                        _ => return Err(error("Invalid endpoint side")),
                    };
                    if p.get("channel").is_some() {
                        c.channel = channel(p, "channel")?;
                    }
                    self.check_connection(&d, &c, Some(i))?;
                    let arrows: Vec<_> = statement
                        .tokens
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| t.text == "-->")
                        .map(|(i, _)| i)
                        .collect();
                    let first = *arrows
                        .first()
                        .ok_or_else(|| error("Missing indexed connection arrow"))?;
                    let last = *arrows.last().unwrap();
                    if side == "start" && c.start != old.start {
                        patches.push(source_index::replace_preserving_comments(
                            &file.text,
                            token_span(&statement.tokens[..first])?,
                            &c.start,
                        )?);
                    }
                    if side == "end" && c.end != old.end {
                        patches.push(source_index::replace_preserving_comments(
                            &file.text,
                            token_span(&statement.tokens[last + 1..statement.tokens.len() - 1])?,
                            &c.end,
                        )?);
                    }
                    if c.channel != old.channel {
                        match (&old.channel, &c.channel) {
                            (Some(_), Some(new)) => {
                                patches.push(source_index::replace_preserving_comments(
                                    &file.text,
                                    token_span(&statement.tokens[first + 1..last])?,
                                    new,
                                )?)
                            }
                            (Some(_), None) => {
                                patches.push(source_index::replace_preserving_comments(
                                    &file.text,
                                    statement.tokens[first + 1].span.start
                                        ..statement.tokens[last].span.end,
                                    "",
                                )?)
                            }
                            (None, Some(new)) => patches.push(source_index::Patch {
                                span: statement.tokens[first].span.clone(),
                                replacement: format!("--> {new} -->"),
                            }),
                            (None, None) => {}
                        }
                    }
                    d.connections[i] = c;
                }
            }
            "set_default" => {
                let name = selected.as_deref().unwrap();
                let param = d
                    .parameters
                    .get_mut(name)
                    .ok_or_else(|| error("Unknown parameter"))?;
                let literal = p.get("literal").ok_or_else(|| error("Missing literal"))?;
                let value = if literal.is_null() {
                    None
                } else {
                    Some(
                        literal
                            .as_str()
                            .ok_or_else(|| error("Literal must be a string or null"))?,
                    )
                };
                if let Some(value) = value {
                    contained_literal(value)?;
                }
                let statement = index
                    .sections
                    .get("parameters")
                    .and_then(|s| {
                        s.statements.iter().find(|s| {
                            s.tokens.get(1).is_some_and(|t| t.text == name)
                                && matches!(
                                    s.tokens[0].text.as_str(),
                                    "int" | "double" | "bool" | "string"
                                )
                        })
                    })
                    .ok_or_else(|| error("Parameter statement is not indexed"))?;
                let eq = statement.tokens.iter().position(|t| t.text == "=");
                let semicolon = statement.tokens.last().unwrap().span.start;
                match (eq, value) {
                    (Some(eq), Some(value)) => {
                        let close = statement
                            .tokens
                            .iter()
                            .enumerate()
                            .skip(eq + 3)
                            .find(|(_, t)| t.text == ")")
                            .map(|(i, _)| i)
                            .ok_or_else(|| error("Default literal boundary is unavailable"))?;
                        patches.push(source_index::replace_preserving_comments(
                            &file.text,
                            token_span(&statement.tokens[eq + 3..close])?,
                            value,
                        )?);
                    }
                    (Some(eq), None) => {
                        let close = statement
                            .tokens
                            .iter()
                            .skip(eq + 3)
                            .find(|t| t.text == ")")
                            .ok_or_else(|| error("Default literal boundary is unavailable"))?;
                        patches.push(source_index::replace_preserving_comments(
                            &file.text,
                            statement.tokens[eq].span.start..close.span.end,
                            "",
                        )?);
                    }
                    (None, Some(value)) => patches.push(source_index::Patch {
                        span: semicolon..semicolon,
                        replacement: format!(" = default({value})"),
                    }),
                    (None, None) => {}
                }
                // Normalize through the same parser literal representation, then check all other structure.
                let candidate = source_index::apply(&file.text, patches.clone())?;
                let package = file
                    .roles
                    .iter()
                    .find_map(|r| {
                        if let FileRole::Ned { package, .. } = r {
                            Some(package.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("");
                let check = crate::input::parse_ned(&candidate, &file.path, package)
                    .map_err(|e| EditorError::common("E-EDITOR-PATCH", e))?;
                let candidate_index = SourceIndex::build(&candidate, &check)?;
                for (old, new) in indexes
                    .declarations
                    .iter()
                    .zip(&candidate_index.declarations)
                {
                    if old.sections.keys().ne(new.sections.keys())
                        || old.sections.iter().any(|(name, section)| {
                            new.sections[name].statements.len() != section.statements.len()
                        })
                    {
                        return Err(error("Default literal must not add or remove statements"));
                    }
                }
                param.default = super::analysis::shapes(&check)[ordinal]
                    .parameters
                    .get(name)
                    .ok_or_else(|| error("Parameter disappeared"))?
                    .default
                    .clone();
            }
            _ => return Err(error("Unknown graph command")),
        }
        expected[ordinal] = d;
        let candidate = source_index::apply(&file.text, patches)?;
        let package = file
            .roles
            .iter()
            .find_map(|r| {
                if let FileRole::Ned { package, .. } = r {
                    Some(package.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");
        let parsed = source_index::check_candidate(&candidate, &file.path, package, &expected)?;
        let result = self.commit(
            vec![(file_id.clone(), candidate)],
            vec![(file_id.clone(), layout)],
        )?;
        self.project.parsed.insert(file_id.clone(), parsed);
        self.parsed_hashes
            .insert(file_id.clone(), self.project.files[&file_id].hash.clone());
        self.syntax.insert(file_id, "parsed".into());
        Ok(result)
    }
}
