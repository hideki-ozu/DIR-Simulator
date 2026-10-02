use super::{
    Result, decimal_parts, error, identifier, quantity, reserved, string_literal, validate_filter,
};
use crate::types::Controller;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug)]
struct Token {
    text: String,
    line: usize,
    column: usize,
}

fn lex(content: &str, path: &Path) -> Result<Vec<Token>> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut tokens = Vec::new();
    let mut cursor = 0;
    let mut line = 1;
    let mut column = 1;
    while cursor < content.len() {
        let start = cursor;
        let start_line = line;
        let start_column = column;
        let rest = &content[cursor..];
        let c = rest.chars().next().unwrap();
        let fail = |message: &str| {
            error(format!(
                "{}:{start_line}:{start_column}: {message}",
                path.display()
            ))
        };
        if matches!(c, ' ' | '\t' | '\n') {
            cursor += 1;
        } else if rest.starts_with("\r\n") {
            cursor += 2;
        } else if rest.starts_with("//") {
            cursor += rest.find('\n').unwrap_or(rest.len());
        } else if let Some(comment) = rest.strip_prefix("/*") {
            let end = comment
                .find("*/")
                .ok_or_else(|| fail("unterminated block comment"))?;
            cursor += end + 4;
        } else {
            if c == '"' {
                let mut escaped = false;
                let mut end = None;
                for (i, c) in rest.char_indices().skip(1) {
                    if !escaped && c == '"' {
                        end = Some(i + 1);
                        break;
                    }
                    if matches!(c, '\n' | '\r') {
                        return Err(fail("newline in string"));
                    }
                    escaped = !escaped && c == '\\';
                }
                cursor += end.ok_or_else(|| fail("unterminated string"))?;
                string_literal(&content[start..cursor]).map_err(|e| fail(&e.message))?;
            } else if rest.starts_with("-->") {
                cursor += 3;
            } else if c.is_ascii_alphabetic() || c == '_' {
                cursor += rest
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    .count();
            } else if c.is_ascii_digit() || c == '-' {
                if c == '-' {
                    cursor += 1;
                }
                let n = content[cursor..]
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count();
                if n == 0 {
                    return Err(fail("unexpected minus"));
                }
                cursor += n;
                if content[cursor..].starts_with('.') {
                    cursor += 1;
                    let n = content[cursor..]
                        .bytes()
                        .take_while(u8::is_ascii_digit)
                        .count();
                    if n == 0 {
                        return Err(fail("missing fractional digits"));
                    }
                    cursor += n;
                }
            } else if matches!(c, '{' | '}' | ':' | ';' | '.' | '@' | '(' | ')' | '=') {
                cursor += 1;
            } else {
                return Err(fail("unsupported NED token"));
            }
            tokens.push(Token {
                text: content[start..cursor].into(),
                line: start_line,
                column: start_column,
            });
        }
        for c in content[start..cursor].chars() {
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        // CR is permitted only as part of CRLF, also inside comments.
        if content[start..cursor].replace("\r\n", "").contains('\r') {
            return Err(fail("bare carriage return"));
        }
    }
    tokens.push(Token {
        text: "<EOF>".into(),
        line,
        column,
    });
    Ok(tokens)
}

#[derive(Clone, Debug)]
struct Parameter {
    scalar: String,
    unit: Option<String>,
    default: Option<String>,
}
#[derive(Clone, Debug)]
struct Connection {
    start: String,
    end: String,
    channel: Option<String>,
}
#[derive(Clone, Debug)]
pub(super) struct Declaration {
    pub name: String,
    kind: String,
    implementation: Option<String>,
    parameters: BTreeMap<String, Parameter>,
    gates: BTreeMap<String, bool>, // true = output
    children: Vec<(String, String)>,
    connections: Vec<Connection>,
    source: String,
}
impl Declaration {
    fn fail(&self, message: impl AsRef<str>) -> crate::types::Diagnostic {
        error(format!(
            "{}: {}: {}",
            self.source,
            self.name,
            message.as_ref()
        ))
    }
    fn simple(&self) -> bool {
        self.kind == "simple"
    }
    fn compound(&self) -> bool {
        matches!(self.kind.as_str(), "network" | "module")
    }
}
struct Parser<'a> {
    tokens: Vec<Token>,
    cursor: usize,
    path: &'a Path,
}
impl Parser<'_> {
    fn peek(&self) -> &str {
        &self.tokens[self.cursor].text
    }
    fn fail(&self, message: impl AsRef<str>) -> crate::types::Diagnostic {
        let token = &self.tokens[self.cursor];
        error(format!(
            "{}:{}:{}: {} (got {})",
            self.path.display(),
            token.line,
            token.column,
            message.as_ref(),
            token.text
        ))
    }
    fn take(&mut self) -> String {
        let value = self.peek().to_string();
        // Keep EOF addressable so every truncated production returns a diagnostic.
        if self.cursor + 1 < self.tokens.len() {
            self.cursor += 1;
        }
        value
    }
    fn eat(&mut self, value: &str) -> bool {
        if self.peek() == value {
            self.take();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, value: &str) -> Result<()> {
        if self.eat(value) {
            Ok(())
        } else {
            Err(self.fail(format!("expected {value}")))
        }
    }
    fn id(&mut self) -> Result<String> {
        if identifier(self.peek()) && !reserved(self.peek()) {
            Ok(self.take())
        } else {
            Err(self.fail("expected identifier"))
        }
    }
    fn name(&mut self, qualified: bool) -> Result<String> {
        let mut name = self.id()?;
        let mut components = 1;
        while self.eat(".") {
            name.push('.');
            name.push_str(&self.id()?);
            components += 1;
        }
        if qualified && components < 2 {
            return Err(self.fail("type reference must be fully qualified"));
        }
        Ok(name)
    }
    fn literal(&mut self) -> Result<String> {
        let mut value = self.take();
        if value.starts_with('"') || matches!(value.as_str(), "true" | "false") {
            return Ok(value);
        }
        if !value.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
            return Err(self.fail("expected literal"));
        }
        if identifier(self.peek()) && !reserved(self.peek()) {
            value.push(' ');
            value.push_str(&self.take());
        }
        Ok(value)
    }
    fn property(&mut self, parameter: bool) -> Result<(String, String)> {
        self.expect("@")?;
        let name = self.id()?;
        if (parameter && !matches!(name.as_str(), "unit" | "display" | "description"))
            || (!parameter && !matches!(name.as_str(), "class" | "display" | "description"))
        {
            return Err(self.fail(format!("unsupported property: {name}")));
        }
        self.expect("(")?;
        let value = if name == "unit" {
            let unit = self.id()?;
            if !matches!(unit.as_str(), "s" | "bps" | "B") {
                return Err(self.fail("unsupported unit"));
            }
            unit
        } else {
            string_literal(&self.take()).map_err(|e| self.fail(e.message))?
        };
        self.expect(")")?;
        Ok((name, value))
    }
    fn declaration(&mut self, package: &str) -> Result<Declaration> {
        let token = &self.tokens[self.cursor];
        let source = format!("{}:{}:{}", self.path.display(), token.line, token.column);
        let kind = self.take();
        if !matches!(kind.as_str(), "simple" | "module" | "network" | "channel") {
            return Err(self.fail("unsupported declaration"));
        }
        let name = format!("{package}.{}", self.id()?);
        self.expect("{")?;
        let mut declaration = Declaration {
            name,
            kind,
            implementation: None,
            parameters: BTreeMap::new(),
            gates: BTreeMap::new(),
            children: Vec::new(),
            connections: Vec::new(),
            source,
        };
        let mut last_section = 0;
        let mut attributes = BTreeSet::new();
        while !self.eat("}") {
            let section = self.take();
            let order = match section.as_str() {
                "parameters" => 1,
                "gates" => 2,
                "submodules" => 3,
                "connections" => 4,
                _ => {
                    return Err(self
                        .fail("expected ordered parameters/gates/submodules/connections section"));
                }
            };
            if order <= last_section
                || (declaration.kind == "channel" && order > 1)
                || (declaration.simple() && order > 2)
            {
                return Err(self.fail("duplicate, out of order, or unsupported section"));
            }
            last_section = order;
            self.expect(":")?;
            while !matches!(
                self.peek(),
                "}" | "parameters" | "gates" | "submodules" | "connections" | "<EOF>"
            ) {
                match order {
                    1 => {
                        if self.peek() == "@" {
                            let (name, value) = self.property(false)?;
                            if !attributes.insert(name.clone()) {
                                return Err(self.fail(format!("duplicate property {name}")));
                            }
                            if name == "class" {
                                if declaration.compound() {
                                    return Err(
                                        self.fail("@class is only supported on simple/channel")
                                    );
                                }
                                declaration.implementation = Some(value);
                            }
                            self.expect(";")?;
                        } else {
                            let scalar = self.take();
                            if !matches!(scalar.as_str(), "int" | "double" | "bool" | "string") {
                                return Err(self.fail("unsupported parameter type"));
                            }
                            let name = self.id()?;
                            let mut unit = None;
                            let mut properties = BTreeSet::new();
                            while self.peek() == "@" {
                                let (property, value) = self.property(true)?;
                                if !properties.insert(property.clone()) {
                                    return Err(self.fail(format!("duplicate property {property}")));
                                }
                                if property == "unit" {
                                    unit = Some(value);
                                }
                            }
                            if unit.is_some() && !matches!(scalar.as_str(), "int" | "double") {
                                return Err(self.fail("unit on nonnumeric parameter"));
                            }
                            let default = if self.eat("=") {
                                self.expect("default")?;
                                self.expect("(")?;
                                let value = self.literal()?;
                                self.expect(")")?;
                                Some(value)
                            } else {
                                None
                            };
                            self.expect(";")?;
                            if declaration
                                .parameters
                                .insert(
                                    name.clone(),
                                    Parameter {
                                        scalar,
                                        unit,
                                        default,
                                    },
                                )
                                .is_some()
                            {
                                return Err(self.fail(format!("duplicate parameter {name}")));
                            }
                        }
                    }
                    2 => {
                        let direction = self.take();
                        if !matches!(direction.as_str(), "input" | "output") {
                            return Err(self.fail("expected input/output scalar gate"));
                        }
                        let name = self.id()?;
                        self.expect(";")?;
                        if declaration
                            .gates
                            .insert(name.clone(), direction == "output")
                            .is_some()
                        {
                            return Err(self.fail(format!("duplicate gate {name}")));
                        }
                    }
                    3 => {
                        let name = self.id()?;
                        self.expect(":")?;
                        let child_type = self.name(true)?;
                        self.expect(";")?;
                        if declaration.children.iter().any(|(other, _)| other == &name) {
                            return Err(self.fail(format!("duplicate child {name}")));
                        }
                        declaration.children.push((name, child_type));
                    }
                    4 => {
                        let start = self.name(false)?;
                        if start.split('.').count() > 2 {
                            return Err(self.fail("endpoint must be direct child.gate or own gate"));
                        }
                        self.expect("-->")?;
                        let middle = self.name(false)?;
                        let (channel, end) = if self.eat("-->") {
                            if !middle.contains('.') {
                                return Err(self.fail("channel must be fully qualified"));
                            }
                            (Some(middle), self.name(false)?)
                        } else {
                            (None, middle)
                        };
                        if end.split('.').count() > 2 {
                            return Err(self.fail("endpoint must be direct child.gate or own gate"));
                        }
                        self.expect(";")?;
                        declaration.connections.push(Connection {
                            start,
                            end,
                            channel,
                        });
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(declaration)
    }
}

pub(super) fn parse(
    content: &str,
    path: &Path,
    expected_package: &str,
) -> Result<Vec<Declaration>> {
    let mut parser = Parser {
        tokens: lex(content, path)?,
        cursor: 0,
        path,
    };
    parser.expect("package")?;
    let package = parser.name(false)?;
    parser.expect(";")?;
    if package != expected_package || expected_package.is_empty() {
        return Err(parser.fail(format!(
            "package {package} does not match parent directory {expected_package}"
        )));
    }
    let mut out = Vec::new();
    while parser.peek() != "<EOF>" {
        out.push(parser.declaration(&package)?);
    }
    if out.is_empty() {
        return Err(parser.fail("NED file has no declarations"));
    }
    Ok(out)
}

#[derive(Clone, Debug)]
enum TypedValue {
    Integer(i64),
    Quantity(u64),
    Double,
    Boolean,
    String(String),
}
fn typed_value(parameter: &Parameter, value: &str) -> Result<TypedValue> {
    match parameter.scalar.as_str() {
        "string" => Ok(TypedValue::String(string_literal(value)?)),
        "bool" if matches!(value, "true" | "false") => Ok(TypedValue::Boolean),
        "int" => {
            if let Some(unit) = &parameter.unit {
                let number = value
                    .trim_start_matches('-')
                    .split([' ', '\t'])
                    .next()
                    .unwrap();
                if number.contains('.') {
                    return Err(error(format!(
                        "int parameter has fractional literal: {value}"
                    )));
                }
                let unsigned_zero = value.strip_prefix('-').filter(|rest| {
                    rest.starts_with('0')
                        && rest
                            .as_bytes()
                            .get(1)
                            .copied()
                            .is_some_and(|c| !c.is_ascii_digit() && c != b'.')
                });
                Ok(TypedValue::Quantity(quantity(
                    unsigned_zero.unwrap_or(value),
                    unit,
                )?))
            } else {
                let digits = value.strip_prefix('-').unwrap_or(value);
                if digits.is_empty()
                    || !digits.bytes().all(|c| c.is_ascii_digit())
                    || (digits.len() > 1 && digits.starts_with('0'))
                {
                    return Err(error(format!("invalid int literal: {value}")));
                }
                Ok(TypedValue::Integer(
                    value
                        .parse()
                        .map_err(|_| error(format!("int exceeds i64: {value}")))?,
                ))
            }
        }
        "double" => {
            if let Some(unit) = &parameter.unit {
                Ok(TypedValue::Quantity(quantity(value, unit)?))
            } else {
                let (integer, fraction, _, rest) = decimal_parts(value, true)?;
                if !rest.is_empty() {
                    return Err(error(format!("unexpected unit or trailing input: {value}")));
                }
                let n: f64 = value
                    .parse()
                    .map_err(|_| error(format!("invalid double: {value}")))?;
                if !n.is_finite()
                    || (n == 0.0 && integer.bytes().chain(fraction.bytes()).any(|b| b != b'0'))
                {
                    return Err(error(format!("double overflow or underflow: {value}")));
                }
                Ok(TypedValue::Double)
            }
        }
        _ => Err(error(format!(
            "invalid {} literal: {value}",
            parameter.scalar
        ))),
    }
}
fn model_value(declaration: &Declaration, name: &str, value: &TypedValue) -> Result<()> {
    let invalid = match (declaration.implementation.as_deref(), name, value) {
        (Some("dir.can.Controller"), "queueCapacity", TypedValue::Integer(n)) => {
            !(0..=4_294_967_295).contains(n)
        }
        (Some("dir.can.Controller"), "rxFilter", TypedValue::String(s)) => {
            validate_filter(s)?;
            false
        }
        (Some("dir.can.Bus"), "bitrate", TypedValue::Quantity(n)) => *n == 0 || *n > 1_000_000,
        (Some("dir.can.Bus"), "profile", TypedValue::String(s)) => s != "can.cc.ideal.v1",
        _ => false,
    };
    if invalid {
        Err(declaration.fail(format!(
            "out of range or unsupported value for {name}: {value:?}"
        )))
    } else {
        Ok(())
    }
}
fn validate_schema(declaration: &Declaration) -> Result<()> {
    let schema: &[(&str, &str, Option<&str>)] = match declaration.implementation.as_deref() {
        Some("dir.can.Controller") if declaration.simple() => &[
            ("queueCapacity", "int", None),
            ("txProcessingDelay", "double", Some("s")),
            ("rxProcessingDelay", "double", Some("s")),
            ("rxFilter", "string", None),
        ],
        Some("dir.can.Bus") if declaration.simple() => &[
            ("bitrate", "double", Some("bps")),
            ("profile", "string", None),
        ],
        Some("dir.link.FixedDelay") if declaration.kind == "channel" => {
            &[("delay", "double", Some("s"))]
        }
        None if declaration.compound() => &[],
        _ => return Err(declaration.fail("missing, unknown, or wrong-kind @class implementation")),
    };
    if !declaration.compound() {
        if declaration.parameters.len() != schema.len() {
            return Err(declaration.fail("parameter schema mismatch"));
        }
        for &(name, scalar, unit) in schema {
            let parameter = declaration
                .parameters
                .get(name)
                .ok_or_else(|| declaration.fail(format!("missing parameter declaration {name}")))?;
            if parameter.scalar != scalar || parameter.unit.as_deref() != unit {
                return Err(declaration.fail(format!("parameter schema mismatch: {name}")));
            }
        }
    }
    match declaration.implementation.as_deref() {
        Some("dir.can.Controller") => {
            if declaration.gates != BTreeMap::from([("tx".into(), true), ("rx".into(), false)]) {
                return Err(declaration.fail("Controller requires output tx and input rx only"));
            }
        }
        Some("dir.can.Bus") => {
            let mut tx = BTreeSet::new();
            let mut rx = BTreeSet::new();
            for (name, output) in &declaration.gates {
                if let Some(suffix) = name.strip_prefix("tx_") {
                    if *output || !identifier(suffix) {
                        return Err(declaration.fail(format!("invalid Bus gate {name}")));
                    }
                    tx.insert(suffix);
                } else if let Some(suffix) = name.strip_prefix("rx_") {
                    if !*output || !identifier(suffix) {
                        return Err(declaration.fail(format!("invalid Bus gate {name}")));
                    }
                    rx.insert(suffix);
                } else {
                    return Err(declaration.fail(format!("invalid Bus gate {name}")));
                }
            }
            if tx != rx || tx.len() < 2 {
                return Err(declaration.fail("Bus needs at least two matching tx_/rx_ gate pairs"));
            }
        }
        _ => {}
    }
    if declaration.kind == "network" && !declaration.gates.is_empty() {
        return Err(declaration.fail("network root gates are unsupported"));
    }
    for (name, parameter) in &declaration.parameters {
        if let Some(value) = &parameter.default {
            let value = typed_value(parameter, value)
                .map_err(|e| declaration.fail(format!("{name} default: {}", e.message)))?;
            model_value(declaration, name, &value).map_err(|e| declaration.fail(e.message))?;
        }
    }
    Ok(())
}

fn endpoint<'a>(
    declaration: &'a Declaration,
    endpoint: &str,
    types: &'a BTreeMap<String, Declaration>,
    start: bool,
) -> Result<()> {
    let (owner, gate, child) = if let Some((child_name, gate)) = endpoint.split_once('.') {
        let child_type = declaration
            .children
            .iter()
            .find(|(name, _)| name == child_name)
            .ok_or_else(|| declaration.fail(format!("unknown child endpoint {endpoint}")))?;
        (&types[&child_type.1], gate, true)
    } else {
        (declaration, endpoint, false)
    };
    let output = owner
        .gates
        .get(gate)
        .ok_or_else(|| declaration.fail(format!("unknown gate {endpoint}")))?;
    if *output != (start == child) {
        return Err(declaration.fail(format!("wrong endpoint direction: {endpoint}")));
    }
    Ok(())
}
fn validate_connections(
    declaration: &Declaration,
    types: &BTreeMap<String, Declaration>,
) -> Result<()> {
    if !declaration.compound() {
        return Ok(());
    }
    let mut used = BTreeSet::new();
    for connection in &declaration.connections {
        endpoint(declaration, &connection.start, types, true)?;
        endpoint(declaration, &connection.end, types, false)?;
        for endpoint in [&connection.start, &connection.end] {
            if !used.insert(endpoint.clone()) {
                return Err(declaration.fail(format!("duplicate connection side: {endpoint}")));
            }
        }
    }
    for gate in declaration.gates.keys() {
        if !used.contains(gate) {
            return Err(declaration.fail(format!("unconnected internal gate: {gate}")));
        }
    }
    for (child, child_type) in &declaration.children {
        for gate in types[child_type].gates.keys() {
            let endpoint = format!("{child}.{gate}");
            if !used.contains(&endpoint) {
                return Err(declaration.fail(format!("unconnected child gate: {endpoint}")));
            }
        }
    }
    Ok(())
}
fn containment(
    name: &str,
    types: &BTreeMap<String, Declaration>,
    stack: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
) -> Result<()> {
    if done.contains(name) {
        return Ok(());
    }
    if !stack.insert(name.into()) {
        return Err(types[name].fail("recursive type containment"));
    }
    for (_, child_type) in &types[name].children {
        containment(child_type, types, stack, done)?;
    }
    stack.remove(name);
    done.insert(name.into());
    Ok(())
}

struct Edge {
    end: String,
    id: String,
    channel: Option<String>,
}
struct Expanded {
    instances: BTreeMap<String, String>,
    edges: BTreeMap<String, Edge>,
}
fn expand(name: &str, path: &str, types: &BTreeMap<String, Declaration>, expanded: &mut Expanded) {
    let declaration = &types[name];
    expanded.instances.insert(path.into(), name.into());
    for connection in &declaration.connections {
        expanded.edges.insert(
            format!("{path}.{}", connection.start),
            Edge {
                end: format!("{path}.{}", connection.end),
                id: format!("{path}::{}", connection.start),
                channel: connection.channel.clone(),
            },
        );
    }
    for (child, child_type) in &declaration.children {
        expand(child_type, &format!("{path}.{child}"), types, expanded);
    }
}
fn expanded(name: &str, types: &BTreeMap<String, Declaration>) -> Expanded {
    let mut result = Expanded {
        instances: BTreeMap::new(),
        edges: BTreeMap::new(),
    };
    expand(name, name.rsplit('.').next().unwrap(), types, &mut result);
    result
}
fn port<'a>(
    endpoint: &'a str,
    expanded: &Expanded,
    types: &BTreeMap<String, Declaration>,
) -> Result<(&'a str, bool)> {
    let (instance, gate) = endpoint
        .rsplit_once('.')
        .ok_or_else(|| error("invalid expanded endpoint"))?;
    let owner = &types[&expanded.instances[instance]];
    Ok((gate, owner.simple()))
}
fn payload(
    endpoint: &str,
    expanded: &Expanded,
    types: &BTreeMap<String, Declaration>,
) -> Option<&'static str> {
    let (instance, gate) = endpoint.rsplit_once('.')?;
    let owner = &types[&expanded.instances[instance]];
    match owner.implementation.as_deref() {
        Some("dir.can.Controller") => Some(if gate == "tx" { "tx" } else { "rx" }),
        Some("dir.can.Bus") => Some(if gate.starts_with("tx_") { "tx" } else { "rx" }),
        _ => None,
    }
}
fn trace<'a>(start: &str, expanded: &'a Expanded) -> Result<(&'a str, Vec<&'a Edge>)> {
    let mut current = start;
    let mut visited = BTreeSet::new();
    let mut edges = Vec::new();
    while let Some(edge) = expanded.edges.get(current) {
        if !visited.insert(current.to_string()) {
            return Err(error(format!("boundary connection cycle from {start}")));
        }
        edges.push(edge);
        current = &edge.end;
    }
    let last = edges
        .last()
        .ok_or_else(|| error(format!("unconnected output: {start}")))?;
    Ok((&last.end, edges))
}
fn validate_paths(expanded: &Expanded, types: &BTreeMap<String, Declaration>) -> Result<()> {
    // Trace every edge as well as simple outputs to reject isolated boundary cycles.
    for start in expanded.edges.keys() {
        let (end, _) = trace(start, expanded)?;
        let (_, simple) = port(end, expanded, types)?;
        if !simple {
            let (instance, gate) = end.rsplit_once('.').unwrap();
            let root = expanded.instances.keys().next().unwrap();
            let declaration = &types[&expanded.instances[instance]];
            if instance != root
                || declaration.kind != "module"
                || declaration.gates.get(gate) != Some(&true)
            {
                return Err(error(format!(
                    "boundary path has no simple sink: {start} --> {end}"
                )));
            }
        }
        if let (Some(source), Some(sink)) = (
            payload(start, expanded, types),
            payload(end, expanded, types),
        ) {
            if source != sink {
                return Err(error(format!(
                    "incompatible CAN payload path: {start} --> {end}"
                )));
            }
        }
    }
    Ok(())
}

fn resolve_values(
    declaration: &Declaration,
    overrides: Option<&BTreeMap<String, String>>,
) -> Result<BTreeMap<String, TypedValue>> {
    if let Some(overrides) = overrides {
        if let Some(key) = overrides
            .keys()
            .find(|key| !declaration.parameters.contains_key(*key))
        {
            return Err(declaration.fail(format!("unknown parameter: {key}")));
        }
    }
    declaration
        .parameters
        .iter()
        .map(|(name, parameter)| {
            let value = overrides
                .and_then(|map| map.get(name))
                .or(parameter.default.as_ref())
                .ok_or_else(|| declaration.fail(format!("missing required parameter: {name}")))?;
            let value = typed_value(parameter, value)
                .map_err(|e| declaration.fail(format!("{name}: {}", e.message)))?;
            model_value(declaration, name, &value)?;
            Ok((name.clone(), value))
        })
        .collect()
}
fn number(values: &BTreeMap<String, TypedValue>, name: &str) -> u64 {
    match &values[name] {
        TypedValue::Integer(n) => *n as u64,
        TypedValue::Quantity(n) => *n,
        _ => unreachable!(),
    }
}
fn text(values: &BTreeMap<String, TypedValue>, name: &str) -> String {
    match &values[name] {
        TypedValue::String(s) => s.clone(),
        _ => unreachable!(),
    }
}
pub(super) struct Resolved {
    pub bus_id: String,
    pub bitrate: u64,
    pub controllers: Vec<Controller>,
    pub channel_count: usize,
}
pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    general: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<Resolved> {
    for declaration in types.values() {
        validate_schema(declaration)?;
        for (_, child_type) in &declaration.children {
            let child = types
                .get(child_type)
                .ok_or_else(|| declaration.fail(format!("unknown child type: {child_type}")))?;
            if !matches!(child.kind.as_str(), "simple" | "module") {
                return Err(declaration.fail(format!("unsupported child type kind: {child_type}")));
            }
        }
        for connection in &declaration.connections {
            if let Some(channel) = &connection.channel {
                if types.get(channel).is_none_or(|d| d.kind != "channel") {
                    return Err(
                        declaration.fail(format!("unknown or wrong-kind channel: {channel}"))
                    );
                }
            }
        }
    }
    let mut done = BTreeSet::new();
    for name in types.keys() {
        containment(name, types, &mut BTreeSet::new(), &mut done)?;
    }
    for declaration in types.values() {
        validate_connections(declaration, types)?;
        if declaration.compound() {
            validate_paths(&expanded(&declaration.name, types), types)?;
        }
    }
    let declaration = types
        .get(network)
        .ok_or_else(|| error(format!("unknown network type: {network}")))?;
    if declaration.kind != "network" {
        return Err(declaration.fail("selected network must have network kind"));
    }
    let expanded = expanded(network, types);
    let mut overrides: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (key, value) in general {
        if matches!(
            key.as_str(),
            "network"
                | "ned-path"
                | "sim-time-limit"
                | "metrics-window"
                | "max-events"
                | "max-delta-cycles"
                | "workload"
                | "model-profile"
                | "model-config"
        ) {
            continue;
        }
        let (instance, parameter) = key
            .rsplit_once('.')
            .ok_or_else(|| error(format!("unknown General key {key}")))?;
        if !expanded.instances.contains_key(instance) {
            return Err(error(format!("unknown instance: {instance}")));
        }
        overrides
            .entry(instance.into())
            .or_default()
            .insert(parameter.into(), value.into());
    }
    let mut values = BTreeMap::new();
    let mut controller_paths = Vec::new();
    let mut buses = Vec::new();
    for (instance, name) in &expanded.instances {
        let declaration = &types[name];
        values.insert(
            instance.clone(),
            resolve_values(declaration, overrides.get(instance))?,
        );
        match declaration.implementation.as_deref() {
            Some("dir.can.Controller") => controller_paths.push(instance.clone()),
            Some("dir.can.Bus") => buses.push(instance.clone()),
            _ => {}
        }
    }
    if buses.len() != 1 || controller_paths.len() < 2 {
        return Err(error(
            "can.cc.ideal.v1 requires exactly one Bus and at least two Controllers",
        ));
    }
    let bus_id = buses.pop().unwrap();
    let mut channel_delays = BTreeMap::new();
    for edge in expanded.edges.values() {
        if let Some(channel) = &edge.channel {
            let values = resolve_values(&types[channel], channels.get(&edge.id))?;
            channel_delays.insert(edge.id.clone(), number(&values, "delay"));
        }
    }
    for id in channels.keys() {
        if !channel_delays.contains_key(id) {
            return Err(error(format!("unknown or channel-less connection: {id}")));
        }
    }
    let delay = |edges: &[&Edge]| -> Result<u64> {
        edges.iter().try_fold(0u64, |sum, edge| {
            sum.checked_add(*channel_delays.get(&edge.id).unwrap_or(&0))
                .ok_or_else(|| error(format!("channel path delay overflow at {}", edge.id)))
        })
    };
    let mut controllers = Vec::new();
    let mut used_suffixes = BTreeSet::new();
    for id in controller_paths {
        let (tx_sink, tx_edges) = trace(&format!("{id}.tx"), &expanded)?;
        let (bus, tx_gate) = tx_sink.rsplit_once('.').unwrap();
        let suffix = tx_gate
            .strip_prefix("tx_")
            .ok_or_else(|| error(format!("Controller {id} tx path must end at Bus tx_SUFFIX")))?;
        if bus != bus_id || !used_suffixes.insert(suffix.to_string()) {
            return Err(error(format!(
                "Controller {id} must connect to the unique Bus gate pair"
            )));
        }
        let (rx_sink, rx_edges) = trace(&format!("{bus_id}.rx_{suffix}"), &expanded)?;
        if rx_sink != format!("{id}.rx") {
            return Err(error(format!(
                "Controller {id} tx/rx must use the same Bus suffix"
            )));
        }
        let values = &values[&id];
        controllers.push(Controller {
            id,
            queue_capacity: number(values, "queueCapacity"),
            tx_processing_ps: number(values, "txProcessingDelay"),
            rx_processing_ps: number(values, "rxProcessingDelay"),
            rx_filter: text(values, "rxFilter"),
            tx_channel_ps: delay(&tx_edges)?,
            rx_channel_ps: delay(&rx_edges)?,
        });
    }
    let bus_type = &types[&expanded.instances[&bus_id]];
    if used_suffixes.len() * 2 != bus_type.gates.len() {
        return Err(error("Bus gate pairs must each connect to one Controller"));
    }
    Ok(Resolved {
        bitrate: number(&values[&bus_id], "bitrate"),
        bus_id,
        controllers,
        channel_count: channel_delays.len(),
    })
}
