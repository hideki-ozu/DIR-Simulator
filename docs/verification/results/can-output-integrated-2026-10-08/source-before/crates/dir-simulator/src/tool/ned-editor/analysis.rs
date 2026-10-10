use super::input::{FileRole, ProjectSnapshot};
use crate::input::{ParsedNed, parse_ned, prepare_with_source};
use crate::types::{Diagnostic, PreparedSimulation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ParameterShape {
    pub scalar: String,
    pub unit: Option<String>,
    pub default: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConnectionShape {
    pub start: String,
    pub end: String,
    pub channel: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeclarationShape {
    pub name: String,
    pub kind: String,
    pub implementation: Option<String>,
    pub parameters: BTreeMap<String, ParameterShape>,
    pub gates: BTreeMap<String, bool>,
    pub children: Vec<(String, String)>,
    pub connections: Vec<ConnectionShape>,
}
pub(crate) fn shapes(parsed: &ParsedNed) -> Vec<DeclarationShape> {
    parsed
        .declarations()
        .iter()
        .map(|d| DeclarationShape {
            name: d.name().to_owned(),
            kind: d.kind().to_owned(),
            implementation: d.implementation().map(str::to_owned),
            parameters: d
                .parameters()
                .iter()
                .map(|(name, p)| {
                    (
                        name.clone(),
                        ParameterShape {
                            scalar: p.scalar().into(),
                            unit: p.unit().map(str::to_owned),
                            default: p.default().map(str::to_owned),
                        },
                    )
                })
                .collect(),
            gates: d.gates().clone(),
            children: d.children().to_vec(),
            connections: d
                .connections()
                .iter()
                .map(|c| ConnectionShape {
                    start: c.start().into(),
                    end: c.end().into(),
                    channel: c.channel().map(str::to_owned),
                })
                .collect(),
        })
        .collect()
}
#[derive(Clone, Debug)]
pub(crate) struct FileAnalysis {
    pub hash: String,
    pub result: std::result::Result<ParsedNed, Diagnostic>,
}
#[derive(Clone, Debug)]
pub(crate) struct AnalysisBatch {
    pub snapshot_id: String,
    pub files: BTreeMap<String, FileAnalysis>,
}
pub(crate) fn parse_project(project: &ProjectSnapshot) -> AnalysisBatch {
    let files = project
        .ned_files()
        .map(|file| {
            let package = file
                .roles
                .iter()
                .find_map(|r| match r {
                    FileRole::Ned { package, .. } => Some(package.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            (
                file.id.clone(),
                FileAnalysis {
                    hash: file.hash.clone(),
                    result: parse_ned(&file.text, &file.path, package),
                },
            )
        })
        .collect();
    AnalysisBatch {
        snapshot_id: project.id.clone(),
        files,
    }
}
pub(crate) fn prepare(
    project: &ProjectSnapshot,
) -> std::result::Result<PreparedSimulation, Diagnostic> {
    prepare_with_source(&project.config, &project.cwd, &project.source())
}
pub(crate) fn type_key(file: &str, ordinal: usize, name: &str) -> String {
    format!("{file}|{ordinal}|{name}")
}
pub(crate) fn node_key(ty: &str, child: &str) -> String {
    format!("{ty}::node::{child}")
}
pub(crate) fn parameter_key(ty: &str, name: &str) -> String {
    format!("{ty}::parameter::{name}")
}
pub(crate) fn connection_key(ty: &str, ordinal: usize) -> String {
    format!("{ty}::connection::{ordinal}")
}
