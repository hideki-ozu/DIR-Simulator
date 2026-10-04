//! Common NED syntax, declarations, typed values, containment and connection paths.
use super::{Result, decimal_parts, error, identifier, quantity, reserved, string_literal};
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
            cursor += rest.find('\n').map_or(rest.len(), |offset| offset + 1);
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
pub struct Parameter {
    scalar: String,
    unit: Option<String>,
    default: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Connection {
    start: String,
    end: String,
    channel: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Declaration {
    pub(super) name: String,
    kind: String,
    implementation: Option<String>,
    parameters: BTreeMap<String, Parameter>,
    gates: BTreeMap<String, bool>, // true = output
    children: Vec<(String, String)>,
    connections: Vec<Connection>,
    source: String,
}
impl Parameter {
    pub fn scalar(&self) -> &str {
        &self.scalar
    }
    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }
    pub fn default(&self) -> Option<&str> {
        self.default.as_deref()
    }
}
impl Connection {
    pub fn start(&self) -> &str {
        &self.start
    }
    pub fn end(&self) -> &str {
        &self.end
    }
    pub fn channel(&self) -> Option<&str> {
        self.channel.as_deref()
    }
}
impl Declaration {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn parameters(&self) -> &BTreeMap<String, Parameter> {
        &self.parameters
    }
    pub fn children(&self) -> &[(String, String)] {
        &self.children
    }
    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    pub(super) fn fail(&self, message: impl AsRef<str>) -> crate::types::Diagnostic {
        error(format!(
            "{}: {}: {}",
            self.source,
            self.name,
            message.as_ref()
        ))
    }
    pub(super) fn simple(&self) -> bool {
        self.kind == "simple"
    }
    pub fn implementation(&self) -> Option<&str> {
        self.implementation.as_deref()
    }
    pub fn gates(&self) -> &BTreeMap<String, bool> {
        &self.gates
    }
    pub(super) fn require_parameters(&self, schema: &[(&str, &str, Option<&str>)]) -> Result<()> {
        if self.parameters.len() != schema.len() {
            return Err(self.fail("parameter schema mismatch"));
        }
        for &(name, scalar, unit) in schema {
            let parameter = self
                .parameters
                .get(name)
                .ok_or_else(|| self.fail(format!("missing parameter declaration {name}")))?;
            if parameter.scalar != scalar || parameter.unit.as_deref() != unit {
                return Err(self.fail(format!("parameter schema mismatch: {name}")));
            }
        }
        Ok(())
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
pub(super) enum TypedValue {
    Integer(i64),
    Quantity(u64),
    Double,
    Boolean,
    String(String),
}
pub(super) fn typed_value(parameter: &Parameter, value: &str) -> Result<TypedValue> {
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
pub(super) type Values = BTreeMap<String, TypedValue>;

/// Model policy for simple declarations; syntax, channels and paths remain common.
pub(super) trait ModelRules {
    fn validate_schema(&self, declaration: &Declaration) -> Result<()>;
    fn validate_value(
        &self,
        declaration: &Declaration,
        name: &str,
        value: &TypedValue,
    ) -> Result<()>;
    fn validate_instance(
        &self,
        _instance: &str,
        _declaration: &Declaration,
        _values: &Values,
    ) -> Result<()> {
        Ok(())
    }
    fn payload(&self, declaration: &Declaration, gate: &str) -> Option<&'static str>;
    fn incompatible_payload(&self, start: &str, end: &str) -> crate::types::Diagnostic {
        error(format!("incompatible payload path: {start} --> {end}"))
    }
}
fn validate_schema(declaration: &Declaration, rules: &impl ModelRules) -> Result<()> {
    match declaration.implementation.as_deref() {
        Some("dir.link.FixedDelay") if declaration.kind == "channel" => {
            declaration.require_parameters(&[("delay", "double", Some("s"))])?
        }
        None if declaration.compound() => {}
        _ => rules.validate_schema(declaration)?,
    }
    if declaration.kind == "network" && !declaration.gates.is_empty() {
        return Err(declaration.fail("network root gates are unsupported"));
    }
    for (name, parameter) in &declaration.parameters {
        if let Some(value) = &parameter.default {
            let value = typed_value(parameter, value)
                .map_err(|e| declaration.fail(format!("{name} default: {}", e.message)))?;
            rules
                .validate_value(declaration, name, &value)
                .map_err(|e| declaration.fail(e.message))?;
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
    rules: &impl ModelRules,
) -> Option<&'static str> {
    let (instance, gate) = endpoint.rsplit_once('.')?;
    rules.payload(&types[&expanded.instances[instance]], gate)
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
fn validate_paths(
    expanded: &Expanded,
    types: &BTreeMap<String, Declaration>,
    rules: &impl ModelRules,
) -> Result<()> {
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
            payload(start, expanded, types, rules),
            payload(end, expanded, types, rules),
        ) {
            if source != sink {
                return Err(rules.incompatible_payload(start, end));
            }
        }
    }
    Ok(())
}

fn resolve_values(
    declaration: &Declaration,
    overrides: Option<&BTreeMap<String, String>>,
    rules: &impl ModelRules,
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
            rules.validate_value(declaration, name, &value)?;
            Ok((name.clone(), value))
        })
        .collect()
}
/// Model-neutral instances and paths. Channel values are resolved separately so
/// an adapter can validate its instance counts before reporting channel errors.
pub(super) struct Resolved<'a> {
    types: &'a BTreeMap<String, Declaration>,
    expanded: Expanded,
    values: BTreeMap<String, Values>,
}
pub(super) struct ResolvedChannels {
    delays: BTreeMap<String, u64>,
    values: BTreeMap<String, Values>,
    implementations: BTreeMap<String, String>,
}
impl ResolvedChannels {
    pub(super) fn len(&self) -> usize {
        self.delays.len()
    }
}
pub(super) struct ResolvedPath<'a> {
    pub end: &'a str,
    edges: Vec<&'a Edge>,
}
impl ResolvedPath<'_> {
    pub(super) fn ethernet_link(&self, channels: &ResolvedChannels) -> Result<(String, u64, u64)> {
        let linked: Vec<_> = self.edges.iter().filter(|e| e.channel.is_some()).collect();
        if linked.len() != 1
            || channels
                .implementations
                .get(&linked[0].id)
                .map(String::as_str)
                != Some("dir.ethernet.Link")
        {
            return Err(error(
                "Ethernet direction requires exactly one Ethernet Link",
            ));
        }
        let values = &channels.values[&linked[0].id];
        let (TypedValue::Quantity(bitrate), TypedValue::Quantity(delay)) =
            (&values["bitrate"], &values["delay"])
        else {
            unreachable!()
        };
        Ok((linked[0].id.clone(), *bitrate, *delay))
    }

    pub(super) fn delay(&self, channels: &ResolvedChannels) -> Result<u64> {
        self.edges.iter().try_fold(0u64, |sum, edge| {
            sum.checked_add(*channels.delays.get(&edge.id).unwrap_or(&0))
                .ok_or_else(|| error(format!("channel path delay overflow at {}", edge.id)))
        })
    }
}
impl Resolved<'_> {
    pub(super) fn instances(&self) -> impl Iterator<Item = (&str, &Declaration)> {
        self.expanded
            .instances
            .iter()
            .map(|(id, name)| (id.as_str(), &self.types[name]))
    }
    pub(super) fn declaration(&self, instance: &str) -> &Declaration {
        &self.types[&self.expanded.instances[instance]]
    }
    pub(super) fn values(&self, instance: &str) -> &Values {
        &self.values[instance]
    }
    pub(super) fn module_paths(&self) -> Vec<String> {
        self.instances()
            .filter(|(_, d)| d.kind == "module")
            .map(|(id, _)| id.to_string())
            .collect()
    }
    pub(super) fn trace(&self, start: &str) -> Result<ResolvedPath<'_>> {
        let (end, edges) = trace(start, &self.expanded)?;
        Ok(ResolvedPath { end, edges })
    }
    pub(super) fn resolve_channels(
        &self,
        channels: &BTreeMap<String, BTreeMap<String, String>>,
        rules: &impl ModelRules,
    ) -> Result<ResolvedChannels> {
        let mut delays = BTreeMap::new();
        let mut channel_values = BTreeMap::new();
        let mut implementations = BTreeMap::new();
        for edge in self.expanded.edges.values() {
            if let Some(channel) = &edge.channel {
                let values = resolve_values(&self.types[channel], channels.get(&edge.id), rules)?;
                let TypedValue::Quantity(delay) = values["delay"] else {
                    unreachable!()
                };
                delays.insert(edge.id.clone(), delay);
                channel_values.insert(edge.id.clone(), values);
                implementations.insert(
                    edge.id.clone(),
                    self.types[channel].implementation().unwrap_or("").into(),
                );
            }
        }
        for id in channels.keys() {
            if !delays.contains_key(id) {
                return Err(error(format!("unknown or channel-less connection: {id}")));
            }
        }
        Ok(ResolvedChannels {
            delays,
            values: channel_values,
            implementations,
        })
    }
}
pub(super) fn resolve<'a>(
    types: &'a BTreeMap<String, Declaration>,
    network: &str,
    assignments: &BTreeMap<String, String>,
    rules: &impl ModelRules,
) -> Result<Resolved<'a>> {
    for declaration in types.values() {
        validate_schema(declaration, rules)?;
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
            validate_paths(&expanded(&declaration.name, types), types, rules)?;
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
    for (key, value) in assignments {
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
    for (instance, name) in &expanded.instances {
        let declaration = &types[name];
        let resolved = resolve_values(declaration, overrides.get(instance), rules)?;
        rules.validate_instance(instance, declaration, &resolved)?;
        values.insert(instance.clone(), resolved);
    }
    Ok(Resolved {
        types,
        expanded,
        values,
    })
}

#[cfg(test)]
mod tests;
