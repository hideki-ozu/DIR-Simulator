//! Read-only, resolved reference evidence from the ordinary preparation pipeline.
use super::{Declaration, Ini, InputSource, JsonDocument, Result, error, ned};
use crate::types::{PreparedSimulation, SourceSpan};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMemberKind {
    Child,
    Gate,
    Parameter,
}

/// Names identify declarations within this validated input, not across edits.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReferenceTarget {
    Type {
        qname: String,
    },
    Member {
        owner: String,
        member_kind: ReferenceMemberKind,
        name: String,
    },
    Instance {
        path: String,
    },
    Channel {
        id: String,
    },
    InstanceParameter {
        path: String,
        owner: String,
        parameter: String,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSyntaxKind {
    TypeDeclaration,
    ChildDeclaration,
    GateDeclaration,
    ParameterDeclaration,
    ChildType,
    ChannelType,
    EndpointChild,
    EndpointGate,
    IniNetwork,
    IniParameterKey,
    IniChannelId,
    IniChannelParameterKey,
    /// The range includes both quotes. Offsets in `segments` refer to decoded UTF-8.
    JsonStringToken,
}
/// One resolved symbol in a decoded path. Offsets are half-open decoded UTF-8 bytes.
#[derive(Clone, Debug, Serialize)]
pub struct ReferenceSegment {
    pub start_byte: usize,
    pub end_byte: usize,
    pub target: ReferenceTarget,
}
#[derive(Clone, Debug, Serialize)]
pub struct ResolvedReferenceUse {
    pub syntax_kind: ReferenceSyntaxKind,
    pub span: SourceSpan,
    /// Qualified NED names expose identifier tokens excluding punctuation and trivia.
    /// Other syntax kinds expose the complete reference token range.
    pub source_segments: Vec<SourceSpan>,
    pub target: ReferenceTarget,
    pub instance_path: Option<String>,
    pub json_pointer: Option<String>,
    pub decoded_value: String,
    pub segments: Vec<ReferenceSegment>,
}
#[derive(Clone, Debug)]
pub struct ReferenceDeclaration {
    pub qname: String,
    pub kind: String,
    pub package: String,
    pub span: SourceSpan,
    pub name_span: SourceSpan,
    /// Includes all member declarations, properties, literals and connections.
    pub declaration: Declaration,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReferenceInstance {
    pub path: String,
    pub type_qname: String,
    pub parent_path: Option<String>,
    pub child_target: Option<ReferenceTarget>,
    pub segments: Vec<ReferenceSegment>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReferenceChannel {
    pub id: String,
    pub type_qname: String,
    pub start_endpoint: String,
    pub end_endpoint: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReferenceUnsupported {
    pub reason: String,
    pub profile: String,
}
#[derive(Clone, Debug)]
pub struct ReferenceInspection {
    pub prepared: PreparedSimulation,
    pub declarations: Vec<ReferenceDeclaration>,
    pub instances: Vec<ReferenceInstance>,
    pub channels: Vec<ReferenceChannel>,
    pub uses: Vec<ResolvedReferenceUse>,
    /// Callers must refuse transformations whenever this is nonempty.
    pub unsupported: Vec<ReferenceUnsupported>,
}

/// Prepare once using exactly the normal acquisition and validation order, while
/// recording resolved reference tokens. No additional reads or writes occur.
pub fn inspect_references_with_source(
    config: &Path,
    cwd: &Path,
    source: &dyn InputSource,
) -> Result<ReferenceInspection> {
    let mut evidence = Evidence::default();
    let prepared = super::prepare_with_source_inner(config, cwd, source, Some(&mut evidence))?;
    let unsupported = if matches!(
        prepared.common.profile.as_str(),
        "can.cc.ideal.v1" | "can.cc.multibus.v1"
    ) {
        Vec::new()
    } else {
        vec![ReferenceUnsupported {
            reason: "reference_profile_unsupported".into(),
            profile: prepared.common.profile.clone(),
        }]
    };
    if !unsupported.is_empty() {
        evidence.uses.clear();
        evidence.instances.clear();
        evidence.channels.clear();
    }
    Ok(ReferenceInspection {
        prepared,
        declarations: evidence.declarations,
        instances: evidence.instances,
        channels: evidence.channels,
        uses: evidence.uses,
        unsupported,
    })
}

#[derive(Default)]
pub(crate) struct Evidence {
    declarations: Vec<ReferenceDeclaration>,
    instances: Vec<ReferenceInstance>,
    channels: Vec<ReferenceChannel>,
    uses: Vec<ResolvedReferenceUse>,
}
fn ty(qname: &str) -> ReferenceTarget {
    ReferenceTarget::Type {
        qname: qname.into(),
    }
}
fn member(owner: &str, kind: ReferenceMemberKind, name: &str) -> ReferenceTarget {
    ReferenceTarget::Member {
        owner: owner.into(),
        member_kind: kind,
        name: name.into(),
    }
}
impl Evidence {
    fn push(
        &mut self,
        kind: ReferenceSyntaxKind,
        span: &SourceSpan,
        target: ReferenceTarget,
        value: &str,
        context: (Option<&str>, Option<&str>),
        segments: Vec<ReferenceSegment>,
    ) {
        self.uses.push(ResolvedReferenceUse {
            syntax_kind: kind,
            span: span.clone(),
            source_segments: vec![span.clone()],
            target,
            instance_path: context.0.map(str::to_owned),
            json_pointer: context.1.map(str::to_owned),
            decoded_value: value.into(),
            segments,
        });
    }
    pub(super) fn declarations(&mut self, types: &BTreeMap<String, Declaration>) {
        self.declarations = types
            .values()
            .map(|d| ReferenceDeclaration {
                qname: d.name().into(),
                kind: d.kind().into(),
                package: d.name().rsplit_once('.').unwrap().0.into(),
                span: d.span().clone(),
                name_span: d.name_span().clone(),
                declaration: d.clone(),
            })
            .collect();
    }
    pub(super) fn topology(
        &mut self,
        types: &BTreeMap<String, Declaration>,
        resolved: &ned::Resolved<'_>,
        ini: &Ini,
    ) {
        use ReferenceMemberKind::{Child, Gate, Parameter};
        use ReferenceSyntaxKind::*;
        for d in types.values() {
            self.push(
                TypeDeclaration,
                d.name_span(),
                ty(d.name()),
                d.name().rsplit('.').next().unwrap(),
                (None, None),
                vec![],
            );
            for (name, p) in d.parameters() {
                self.push(
                    ParameterDeclaration,
                    p.name_span(),
                    member(d.name(), Parameter, name),
                    name,
                    (None, None),
                    vec![],
                );
            }
            for name in d.gates().keys() {
                self.push(
                    GateDeclaration,
                    d.gate_span(name).unwrap(),
                    member(d.name(), Gate, name),
                    name,
                    (None, None),
                    vec![],
                );
            }
            for (name, qname) in d.children() {
                self.push(
                    ChildDeclaration,
                    d.child_name_span(name).unwrap(),
                    member(d.name(), Child, name),
                    name,
                    (None, None),
                    vec![],
                );
                self.push(
                    ChildType,
                    d.child_type_span(name).unwrap(),
                    ty(qname),
                    qname,
                    (None, None),
                    vec![],
                );
                self.uses.last_mut().unwrap().source_segments =
                    d.child_type_segments(name).unwrap().to_vec();
            }
            for c in d.connections() {
                if let Some(qname) = c.channel() {
                    self.push(
                        ChannelType,
                        c.channel_span().unwrap(),
                        ty(qname),
                        qname,
                        (None, None),
                        vec![],
                    );
                    self.uses.last_mut().unwrap().source_segments =
                        c.channel_segments().unwrap().to_vec();
                }
                for (endpoint, spans) in
                    [(c.start(), c.start_segments()), (c.end(), c.end_segments())]
                {
                    if let Some((child, gate)) = endpoint.split_once('.') {
                        let owner = d
                            .children()
                            .iter()
                            .find(|(name, _)| name == child)
                            .unwrap()
                            .1
                            .as_str();
                        self.push(
                            EndpointChild,
                            &spans[0],
                            member(d.name(), Child, child),
                            child,
                            (None, None),
                            vec![],
                        );
                        self.push(
                            EndpointGate,
                            &spans[1],
                            member(owner, Gate, gate),
                            gate,
                            (None, None),
                            vec![],
                        );
                    } else {
                        self.push(
                            EndpointGate,
                            &spans[0],
                            member(d.name(), Gate, endpoint),
                            endpoint,
                            (None, None),
                            vec![],
                        );
                    }
                }
            }
        }
        self.channels = resolved
            .reference_channels()
            .map(|(id, d, start, end)| ReferenceChannel {
                id: id.into(),
                type_qname: d.name().into(),
                start_endpoint: start.into(),
                end_endpoint: end.into(),
            })
            .collect();
        let instance_types: BTreeMap<_, _> = resolved
            .instances()
            .map(|(path, d)| (path.to_owned(), d.name().to_owned()))
            .collect();
        self.instances = instance_types
            .iter()
            .map(|(path, qname)| {
                let parent = path.rsplit_once('.').map(|(parent, _)| parent.to_owned());
                let child_target = parent.as_ref().map(|parent| {
                    member(
                        &instance_types[parent],
                        Child,
                        path.rsplit('.').next().unwrap(),
                    )
                });
                ReferenceInstance {
                    path: path.clone(),
                    type_qname: qname.clone(),
                    parent_path: parent,
                    child_target,
                    segments: path_segments(path, &instance_types),
                }
            })
            .collect();
        let network = &ini.general["network"];
        self.push(
            IniNetwork,
            &ini.spans["network"].1,
            ty(network),
            network,
            (None, None),
            vec![],
        );
        for (key, (span, _)) in &ini.spans {
            if ini.general.contains_key(key) && key.contains('.') {
                let (path, parameter) = key.rsplit_once('.').unwrap();
                let owner = &instance_types[path];
                let mut segments = path_segments(path, &instance_types);
                segments.push(ReferenceSegment {
                    start_byte: path.len() + 1,
                    end_byte: key.len(),
                    target: member(owner, Parameter, parameter),
                });
                self.push(
                    IniParameterKey,
                    span,
                    ReferenceTarget::InstanceParameter {
                        path: path.into(),
                        owner: owner.clone(),
                        parameter: parameter.into(),
                    },
                    key,
                    (Some(path), None),
                    segments,
                );
            }
        }
        let channels: BTreeMap<_, _> = resolved.channels().into_iter().collect();
        for (id, span) in &ini.section_spans {
            let (path, endpoint) = id.split_once("::").unwrap();
            let owner = &types[&instance_types[path]];
            let mut segments = path_segments(path, &instance_types);
            let base = path.len() + 2;
            if let Some((child, gate)) = endpoint.split_once('.') {
                segments.push(ReferenceSegment {
                    start_byte: base,
                    end_byte: base + child.len(),
                    target: member(owner.name(), Child, child),
                });
                let child_type = &owner
                    .children()
                    .iter()
                    .find(|(name, _)| name == child)
                    .unwrap()
                    .1;
                segments.push(ReferenceSegment {
                    start_byte: base + child.len() + 1,
                    end_byte: id.len(),
                    target: member(child_type, Gate, gate),
                });
            } else {
                segments.push(ReferenceSegment {
                    start_byte: base,
                    end_byte: id.len(),
                    target: member(owner.name(), Gate, endpoint),
                });
            }
            self.push(
                IniChannelId,
                span,
                ReferenceTarget::Channel { id: id.clone() },
                id,
                (Some(path), None),
                segments,
            );
            for key in ini.channels[id].keys() {
                let full_key = format!("{id}.{key}");
                self.push(
                    IniChannelParameterKey,
                    &ini.spans[&full_key].0,
                    member(channels[id.as_str()].name(), Parameter, key),
                    key,
                    (Some(id), None),
                    vec![],
                );
            }
        }
    }
    pub(super) fn json_instance(
        &mut self,
        document: &JsonDocument<'_>,
        path: &Path,
        pointer: &str,
        decoded: &str,
    ) -> Result<()> {
        let instance = self
            .instances
            .iter()
            .find(|instance| instance.path == decoded)
            .ok_or_else(|| {
                error("resolved reference instance has no topology evidence")
                    .with_reason("reference_unresolved")
            })?;
        let segments = instance.segments.clone();
        let span = document.span(path, pointer).ok_or_else(|| {
            error("resolved reference has no source span").with_reason("reference_span_unsupported")
        })?;
        self.push(
            ReferenceSyntaxKind::JsonStringToken,
            &span,
            ReferenceTarget::Instance {
                path: decoded.into(),
            },
            decoded,
            (Some(decoded), Some(pointer)),
            segments,
        );
        Ok(())
    }
}
fn path_segments(path: &str, types: &BTreeMap<String, String>) -> Vec<ReferenceSegment> {
    let mut result = Vec::new();
    let mut offset = 0;
    for segment in path.split('.') {
        let end = offset + segment.len();
        let target = if offset == 0 {
            ty(&types[segment])
        } else {
            member(
                &types[&path[..offset - 1]],
                ReferenceMemberKind::Child,
                segment,
            )
        };
        result.push(ReferenceSegment {
            start_byte: offset,
            end_byte: end,
            target,
        });
        offset = end + 1;
    }
    result
}

#[cfg(test)]
mod tests;
