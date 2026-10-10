//! Source spans are editor-only. The common parser remains the syntax authority.
use super::analysis::{DeclarationShape, shapes};
use super::{EditorError, Result};
use crate::input::{ParsedNed, parse_ned};
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub text: String,
    pub span: Range<usize>,
}
#[derive(Clone, Debug)]
pub(crate) struct Statement {
    pub tokens: Vec<Token>,
    pub span: Range<usize>,
}
#[derive(Clone, Debug)]
pub(crate) struct Section {
    pub start: usize,
    pub end: usize,
    pub statements: Vec<Statement>,
}
#[derive(Clone, Debug)]
pub(crate) struct DeclarationIndex {
    pub body_end: usize,
    pub sections: BTreeMap<String, Section>,
}
#[derive(Clone, Debug)]
pub(crate) struct SourceIndex {
    pub declarations: Vec<DeclarationIndex>,
}
#[derive(Clone, Debug)]
pub(crate) struct Patch {
    pub span: Range<usize>,
    pub replacement: String,
}
fn unmapped(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-UNMAPPED", message, 422)
}

pub(crate) fn tokens(source: &str) -> Result<Vec<Token>> {
    let mut out = Vec::new();
    let mut i = if source.starts_with('\u{feff}') { 3 } else { 0 };
    while i < source.len() {
        let rest = &source[i..];
        let c = rest.chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
            continue;
        }
        if let Some(comment) = rest.strip_prefix("/*") {
            i += comment
                .find("*/")
                .ok_or_else(|| unmapped("Unterminated comment"))?
                + 4;
            continue;
        }
        let start = i;
        if c == '"' {
            i += 1;
            let mut escaped = false;
            let mut closed = false;
            while i < source.len() {
                let ch = source[i..].chars().next().unwrap();
                i += ch.len_utf8();
                if !escaped && ch == '"' {
                    closed = true;
                    break;
                }
                escaped = ch == '\\' && !escaped;
            }
            if !closed {
                return Err(unmapped("Unterminated string"));
            }
        } else if rest.starts_with("-->") {
            i += 3;
        } else if c.is_ascii_alphabetic() || c == '_' {
            i += rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
        } else if c.is_ascii_digit() || c == '-' {
            if c == '-' {
                i += 1;
            }
            i += source[i..].bytes().take_while(u8::is_ascii_digit).count();
            if source[i..].starts_with('.') {
                i += 1;
                i += source[i..].bytes().take_while(u8::is_ascii_digit).count();
            }
        } else {
            i += c.len_utf8();
        }
        if i == start {
            return Err(unmapped("Cannot advance source scanner"));
        }
        out.push(Token {
            text: source[start..i].into(),
            span: start..i,
        });
    }
    Ok(out)
}
impl SourceIndex {
    pub fn build(source: &str, parsed: &ParsedNed) -> Result<Self> {
        let tokens = tokens(source)?;
        let mut indexes = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            if !matches!(
                tokens[i].text.as_str(),
                "simple" | "module" | "network" | "channel"
            ) {
                i += 1;
                continue;
            }
            let name = tokens
                .get(i + 1)
                .ok_or_else(|| unmapped("Missing declaration name"))?;
            let expected = parsed
                .declarations()
                .get(indexes.len())
                .ok_or_else(|| unmapped("Unexpected declaration"))?;
            if expected.name().rsplit('.').next() != Some(name.text.as_str())
                || expected.kind() != tokens[i].text
            {
                return Err(unmapped("Declaration index differs from shared parser"));
            }
            let open = tokens
                .get(i + 2)
                .ok_or_else(|| unmapped("Missing declaration body"))?;
            if open.text != "{" {
                return Err(unmapped("Missing body brace"));
            }
            let mut end = i + 3;
            let mut depth = 1;
            while end < tokens.len() {
                match tokens[end].text.as_str() {
                    "{" => depth += 1,
                    "}" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                end += 1;
            }
            if end == tokens.len() {
                return Err(unmapped("Missing closing brace"));
            }
            let mut sections = BTreeMap::new();
            let mut cursor = i + 3;
            while cursor < end {
                let header = tokens[cursor].text.clone();
                if !matches!(
                    header.as_str(),
                    "parameters" | "gates" | "submodules" | "connections"
                ) || tokens.get(cursor + 1).map(|t| t.text.as_str()) != Some(":")
                {
                    return Err(unmapped("Unsupported section boundary"));
                }
                let header_start = tokens[cursor].span.start;
                cursor += 2;
                let mut statements = Vec::new();
                let mut section_end = tokens[end].span.start;
                while cursor < end {
                    if matches!(
                        tokens[cursor].text.as_str(),
                        "parameters" | "gates" | "submodules" | "connections"
                    ) && tokens.get(cursor + 1).map(|t| t.text.as_str()) == Some(":")
                    {
                        section_end = tokens[cursor].span.start;
                        break;
                    }
                    let start = cursor;
                    let mut parens = 0;
                    while cursor < end {
                        match tokens[cursor].text.as_str() {
                            "(" => parens += 1,
                            ")" => parens -= 1,
                            ";" if parens == 0 => break,
                            _ => {}
                        }
                        cursor += 1;
                    }
                    if cursor >= end {
                        return Err(unmapped("Missing statement terminator"));
                    }
                    statements.push(Statement {
                        tokens: tokens[start..=cursor].to_vec(),
                        span: tokens[start].span.start..tokens[cursor].span.end,
                    });
                    cursor += 1;
                }
                sections.insert(
                    header,
                    Section {
                        start: header_start,
                        end: section_end,
                        statements,
                    },
                );
            }
            indexes.push(DeclarationIndex {
                body_end: tokens[end].span.start,
                sections,
            });
            i = end + 1;
        }
        if indexes.len() != parsed.declarations().len() {
            return Err(unmapped("Declaration count differs from shared parser"));
        }
        Ok(Self {
            declarations: indexes,
        })
    }
}
pub(crate) fn newline(source: &str) -> &'static str {
    let crlf = source.matches("\r\n").count();
    let lf = source.matches('\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}
fn retained_comments(source: &str) -> Result<String> {
    let mut out = String::new();
    let mut i = 0;
    let nl = newline(source);
    while i < source.len() {
        let rest = &source[i..];
        let c = rest.chars().next().unwrap();
        if c == '"' {
            i += 1;
            let mut escaped = false;
            while i < source.len() {
                let ch = source[i..].chars().next().unwrap();
                i += ch.len_utf8();
                if ch == '"' && !escaped {
                    break;
                }
                escaped = ch == '\\' && !escaped;
            }
        } else if rest.starts_with("//") {
            let n = rest.find('\n').map_or(rest.len(), |n| n + 1);
            out.push_str(&rest[..n]);
            if !rest[..n].ends_with('\n') {
                out.push_str(nl);
            }
            i += n;
        } else if let Some(comment) = rest.strip_prefix("/*") {
            let n = comment
                .find("*/")
                .ok_or_else(|| unmapped("Unterminated comment"))?
                + 4;
            out.push_str(&rest[..n]);
            out.push_str(nl);
            i += n;
        } else {
            i += c.len_utf8();
        }
    }
    Ok(out)
}
pub(crate) fn delete_statement(source: &str, statement: &Statement) -> Result<Patch> {
    Ok(Patch {
        span: statement.span.clone(),
        replacement: retained_comments(&source[statement.span.clone()])?,
    })
}
pub(crate) fn replace_preserving_comments(
    source: &str,
    span: Range<usize>,
    text: &str,
) -> Result<Patch> {
    let comments = retained_comments(&source[span.clone()])?;
    let replacement = if comments.is_empty() {
        text.into()
    } else {
        format!("{text} {}{comments}", newline(source))
    };
    Ok(Patch { span, replacement })
}
pub(crate) fn insert_statement(
    source: &str,
    index: &DeclarationIndex,
    section: &str,
    statement: &str,
) -> Result<Patch> {
    let nl = newline(source);
    let order = ["parameters", "gates", "submodules", "connections"];
    let order_index = order
        .iter()
        .position(|s| *s == section)
        .ok_or_else(|| unmapped("Unknown section"))?;
    let insertion = index
        .sections
        .get(section)
        .map(|s| s.end)
        .unwrap_or_else(|| {
            order[order_index + 1..]
                .iter()
                .find_map(|s| index.sections.get(*s).map(|v| v.start))
                .unwrap_or(index.body_end)
        });
    let indent = index
        .sections
        .get(section)
        .and_then(|s| s.statements.first())
        .map(|s| {
            let start = source[..s.span.start].rfind('\n').map_or(0, |n| n + 1);
            let prefix = &source[start..s.span.start];
            if prefix.chars().all(|c| matches!(c, ' ' | '\t')) {
                prefix.to_owned()
            } else {
                "    ".into()
            }
        })
        .unwrap_or_else(|| "    ".into());
    let replacement = if index.sections.contains_key(section) {
        format!("{nl}{indent}{statement}{nl}")
    } else {
        format!("{nl}{section}:{nl}{indent}{statement}{nl}")
    };
    Ok(Patch {
        span: insertion..insertion,
        replacement,
    })
}
pub(crate) fn apply(source: &str, mut patches: Vec<Patch>) -> Result<String> {
    patches.sort_by_key(|p| p.span.start);
    for p in &patches {
        if p.span.start > p.span.end
            || p.span.end > source.len()
            || !source.is_char_boundary(p.span.start)
            || !source.is_char_boundary(p.span.end)
        {
            return Err(EditorError::new(
                "E-EDITOR-PATCH",
                "Invalid UTF-8 patch boundary",
                422,
            ));
        }
    }
    for pair in patches.windows(2) {
        if pair[0].span.end > pair[1].span.start || pair[0].span.start == pair[1].span.start {
            return Err(EditorError::new(
                "E-EDITOR-PATCH",
                "Overlapping source patches",
                422,
            ));
        }
    }
    let mut result = source.to_owned();
    for p in patches.into_iter().rev() {
        result.replace_range(p.span, &p.replacement);
    }
    Ok(result)
}
pub(crate) fn check_candidate(
    source: &str,
    path: &Path,
    package: &str,
    expected: &[DeclarationShape],
) -> Result<ParsedNed> {
    let parsed =
        parse_ned(source, path, package).map_err(|e| EditorError::common("E-EDITOR-PATCH", e))?;
    if shapes(&parsed) != expected {
        return Err(EditorError::new(
            "E-EDITOR-PATCH",
            "Candidate changed more than the requested structure",
            422,
        ));
    }
    SourceIndex::build(source, &parsed)?;
    Ok(parsed)
}
