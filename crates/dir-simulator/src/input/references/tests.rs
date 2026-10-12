use super::*;
use crate::input::{FsInputSource, InputDirEntry, InputKind, InputMetadata, prepare_with_source};
use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

struct Recorded<'a> {
    source: &'a dyn InputSource,
    calls: RefCell<Vec<(String, PathBuf)>>,
}
impl<'a> Recorded<'a> {
    fn new(source: &'a dyn InputSource) -> Self {
        Self {
            source,
            calls: RefCell::default(),
        }
    }
    fn record(&self, operation: &str, path: &Path) {
        self.calls
            .borrow_mut()
            .push((operation.into(), path.into()));
    }
}
impl InputSource for Recorded<'_> {
    fn metadata(&self, p: &Path) -> io::Result<InputMetadata> {
        self.record("metadata", p);
        self.source.metadata(p)
    }
    fn read_dir(&self, p: &Path) -> io::Result<Vec<InputDirEntry>> {
        self.record("read_dir", p);
        self.source.read_dir(p)
    }
    fn read_utf8(&self, p: &Path) -> io::Result<String> {
        self.record("read_utf8", p);
        self.source.read_utf8(p)
    }
}
struct Snapshot {
    files: BTreeMap<PathBuf, String>,
    directories: BTreeMap<PathBuf, BTreeMap<OsString, InputKind>>,
}
impl Snapshot {
    fn from_prepared(prepared: &PreparedSimulation) -> Self {
        let files: BTreeMap<_, _> = prepared
            .common
            .inputs
            .iter()
            .map(|f| (f.path.clone(), f.content.clone()))
            .collect();
        let mut directories: BTreeMap<PathBuf, BTreeMap<OsString, InputKind>> = BTreeMap::new();
        for path in files.keys() {
            for parent in path.parent().unwrap().ancestors() {
                directories.entry(parent.into()).or_default();
                if let (Some(ancestor), Some(name)) = (parent.parent(), parent.file_name()) {
                    directories
                        .entry(ancestor.into())
                        .or_default()
                        .insert(name.into(), InputKind::Directory);
                }
            }
            directories
                .get_mut(path.parent().unwrap())
                .unwrap()
                .insert(path.file_name().unwrap().into(), InputKind::File);
        }
        Self { files, directories }
    }
}
impl InputSource for Snapshot {
    fn metadata(&self, p: &Path) -> io::Result<InputMetadata> {
        let kind = if self.files.contains_key(p) {
            InputKind::File
        } else if self.directories.contains_key(p) {
            InputKind::Directory
        } else {
            return Err(io::ErrorKind::NotFound.into());
        };
        Ok(InputMetadata { kind })
    }
    fn read_dir(&self, p: &Path) -> io::Result<Vec<InputDirEntry>> {
        self.directories
            .get(p)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(name, kind)| InputDirEntry {
                        name: name.clone(),
                        kind: Ok(*kind),
                    })
                    .collect()
            })
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
    fn read_utf8(&self, p: &Path) -> io::Result<String> {
        self.files
            .get(p)
            .cloned()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn shape(i: &ReferenceInspection) -> serde_json::Value {
    serde_json::json!({"uses": i.uses, "instances": i.instances, "channels": i.channels, "unsupported": i.unsupported})
}
fn raw<'a>(i: &'a ReferenceInspection, u: &ResolvedReferenceUse) -> &'a str {
    let f = i
        .prepared
        .common
        .inputs
        .iter()
        .find(|f| f.path.to_string_lossy() == u.span.source)
        .unwrap();
    &f.content[u.span.start_byte..u.span.end_byte]
}
#[test]
fn references_fs_snapshot_and_ordinary_prepare_preserve_acquisition_and_results() {
    let cwd = root();
    for name in [
        "examples/can/baseline.ini",
        "examples/gateway/fanout.ini",
        "examples/gateway/buffered-fanout.ini",
    ] {
        let ordinary_source = Recorded::new(&FsInputSource);
        let inspected_source = Recorded::new(&FsInputSource);
        let ordinary = prepare_with_source(Path::new(name), &cwd, &ordinary_source).unwrap();
        let inspected =
            inspect_references_with_source(Path::new(name), &cwd, &inspected_source).unwrap();
        assert_eq!(
            *ordinary_source.calls.borrow(),
            *inspected_source.calls.borrow(),
            "{name}"
        );
        assert_eq!(
            format!("{ordinary:?}"),
            format!("{:?}", inspected.prepared),
            "{name}"
        );
        let snapshot = Snapshot::from_prepared(&ordinary);
        let replayed = inspect_references_with_source(Path::new(name), &cwd, &snapshot).unwrap();
        assert_eq!(shape(&inspected), shape(&replayed));
        assert_eq!(
            format!("{:?}", inspected.declarations),
            format!("{:?}", replayed.declarations)
        );
        assert!(inspected.unsupported.is_empty());
        assert!(
            inspected
                .uses
                .iter()
                .all(|u| u.span.start_byte < u.span.end_byte)
        );
        for u in &inspected.uses {
            if u.syntax_kind == ReferenceSyntaxKind::JsonStringToken {
                assert_eq!(
                    serde_json::from_str::<String>(raw(&inspected, u)).unwrap(),
                    u.decoded_value
                );
            } else {
                assert_eq!(raw(&inspected, u), u.decoded_value);
            }
        }
    }
}
#[test]
fn references_include_unused_types_all_members_endpoints_and_required_json_fields() {
    let cwd = root();
    let i = inspect_references_with_source(
        Path::new("examples/gateway/fanout.ini"),
        &cwd,
        &FsInputSource,
    )
    .unwrap();
    assert!(
        !i.instances
            .iter()
            .any(|instance| instance.type_qname == "gateway.Gateway")
    );
    assert!(i.declarations.iter().any(|d| d.qname == "gateway.Gateway"));
    for d in &i.declarations {
        for (name, p) in d.declaration.parameters() {
            assert!(i.uses.iter().any(|u| u.target
                == member(&d.qname, ReferenceMemberKind::Parameter, name)
                && u.span.start_byte == p.name_span().start_byte));
        }
        for name in d.declaration.gates().keys() {
            assert!(i.uses.iter().any(|u| u.target
                == member(&d.qname, ReferenceMemberKind::Gate, name)
                && u.syntax_kind == ReferenceSyntaxKind::GateDeclaration));
        }
        for (child, qname) in d.declaration.children() {
            assert!(i.uses.iter().any(|u| u.target
                == member(&d.qname, ReferenceMemberKind::Child, child)
                && u.syntax_kind == ReferenceSyntaxKind::ChildDeclaration));
            assert!(i.uses.iter().any(|u| u.target == ty(qname)
                && u.span.start_byte == d.declaration.child_type_span(child).unwrap().start_byte
                && u.span.source == d.span.source));
        }
    }
    assert!(i.uses.iter().any(|u| u.target
        == member("gateway.Gateway", ReferenceMemberKind::Gate, "rx_a")
        && u.syntax_kind == ReferenceSyntaxKind::EndpointGate));
    for pointer in [
        "/generators/0/node",
        "/generators/1/node",
        "/gateways/0/node",
        "/gateways/0/ports/0",
        "/gateways/0/ports/1",
        "/gateways/0/ports/2",
        "/gateways/0/routes/0/ingress",
        "/gateways/0/routes/0/egress/0",
        "/gateways/0/routes/0/egress/1",
    ] {
        assert!(
            i.uses
                .iter()
                .any(|u| u.json_pointer.as_deref() == Some(pointer)),
            "{pointer}"
        );
    }
    assert!(i.uses.iter().all(|u| {
        u.json_pointer
            .as_deref()
            .is_none_or(|p| !p.ends_with("/id") && !p.ends_with("/kind") && !p.ends_with("/data"))
    }));
}
#[test]
fn references_bom_crlf_escaped_json_and_channel_spans_keep_nonreferences_untouched() {
    let cwd = root();
    let ordinary = prepare_with_source(
        Path::new("examples/gateway/fanout.ini"),
        &cwd,
        &FsInputSource,
    )
    .unwrap();
    let mut snapshot = Snapshot::from_prepared(&ordinary);
    let config = ordinary.common.config_path.clone();
    snapshot
        .files
        .get_mut(&config)
        .unwrap()
        .push_str("\n[Channel Main::src.tx]\ndelay = 1us\n");
    for (path, text) in &mut snapshot.files {
        if path.extension().is_some_and(|e| e == "ned") {
            *text = text.replace("@class(\"dir.can.MultibusController\");", "@class(\"dir.can.MultibusController\");\n  @description(\"Main.gw.a\"); // 日本語 Main.gw.a");
        }
        if path.extension().is_some_and(|e| e == "json") {
            *text = text
                .replace("Main.", "Main\\u002e")
                .replace("\"node\"", "\"n\\u006fde\"");
        }
        *text = format!("\u{feff}{}", text.replace('\n', "\r\n"));
    }
    let i = inspect_references_with_source(&config, &cwd, &snapshot).unwrap();
    let channel = i
        .uses
        .iter()
        .find(|u| u.syntax_kind == ReferenceSyntaxKind::IniChannelId)
        .unwrap();
    assert_eq!(raw(&i, channel), "Main::src.tx");
    assert_eq!(
        channel
            .segments
            .iter()
            .map(|s| channel.decoded_value[s.start_byte..s.end_byte].to_owned())
            .collect::<Vec<_>>(),
        ["Main", "src", "tx"]
    );
    assert!(i.uses.iter().any(
        |u| u.syntax_kind == ReferenceSyntaxKind::IniChannelParameterKey
            && u.target == member("gateway.Wire", ReferenceMemberKind::Parameter, "delay")
    ));
    for u in &i.uses {
        if u.syntax_kind == ReferenceSyntaxKind::JsonStringToken {
            assert_eq!(
                serde_json::from_str::<String>(raw(&i, u)).unwrap(),
                u.decoded_value
            );
            assert!(raw(&i, u).contains("\\u002e"));
        } else {
            assert_eq!(raw(&i, u), u.decoded_value);
        }
        for segment in &u.segments {
            assert!(
                u.decoded_value.is_char_boundary(segment.start_byte)
                    && u.decoded_value.is_char_boundary(segment.end_byte)
            );
        }
    }
    for d in &i.declarations {
        for attribute in d.declaration.attributes().values() {
            assert!(
                !i.uses
                    .iter()
                    .any(|u| u.span.source == attribute.span().source
                        && u.span.start_byte >= attribute.span().start_byte
                        && u.span.end_byte <= attribute.span().end_byte)
            );
        }
    }
}
#[test]
fn references_rejections_match_prepare_without_additional_acquisition() {
    let cwd = root();
    let ordinary = prepare_with_source(
        Path::new("examples/gateway/fanout.ini"),
        &cwd,
        &FsInputSource,
    )
    .unwrap();
    for mutation in [
        "unknown_profile",
        "unknown_member",
        "unknown_node",
        "unknown_type",
        "unknown_unused_endpoint",
        "unknown_unused_channel",
        "unused_cycle",
        "duplicate_json",
        "unsupported_schema",
    ] {
        let mut snapshot = Snapshot::from_prepared(&ordinary);
        match mutation {
            "unknown_profile" => {
                let text = snapshot
                    .files
                    .get_mut(&ordinary.common.config_path)
                    .unwrap();
                *text = text.replace("can.cc.multibus.v1", "not.registered.v1");
            }
            "unknown_member" => {
                let text = snapshot
                    .files
                    .get_mut(&ordinary.common.config_path)
                    .unwrap();
                *text = text.replace("Main.gw.b.queueCapacity", "Main.gw.b.missing");
            }
            "unknown_node" => {
                let text = snapshot
                    .files
                    .get_mut(ordinary.common.workload_path.as_ref().unwrap())
                    .unwrap();
                *text = text.replace("Main.src", "Main.missing");
            }
            "unknown_type" => {
                let text = snapshot
                    .files
                    .iter_mut()
                    .find(|(p, _)| p.ends_with("Types.ned"))
                    .unwrap()
                    .1;
                *text = text.replace("a: gateway.Controller", "a: gateway.Missing");
            }
            "unknown_unused_endpoint" => {
                let text = snapshot
                    .files
                    .iter_mut()
                    .find(|(p, _)| p.ends_with("Types.ned"))
                    .unwrap()
                    .1;
                *text = text.replacen("rx_a --> a.rx;", "rx_a --> missing.rx;", 1);
            }
            "unknown_unused_channel" => {
                let text = snapshot
                    .files
                    .iter_mut()
                    .find(|(p, _)| p.ends_with("Types.ned"))
                    .unwrap()
                    .1;
                *text = text.replacen(
                    "rx_a --> a.rx;",
                    "rx_a --> gateway.MissingWire --> a.rx;",
                    1,
                );
            }
            "unused_cycle" => {
                let text = snapshot
                    .files
                    .iter_mut()
                    .find(|(p, _)| p.ends_with("Types.ned"))
                    .unwrap()
                    .1;
                *text = text.replacen("a: gateway.Controller;", "a: gateway.Gateway;", 1);
            }
            "duplicate_json" => {
                let text = snapshot
                    .files
                    .get_mut(ordinary.common.workload_path.as_ref().unwrap())
                    .unwrap();
                *text = text.replace(
                    "\"node\": \"Main.src\"",
                    "\"node\": \"Main.src\", \"node\": \"Main.src\"",
                );
            }
            "unsupported_schema" => {
                let text = snapshot
                    .files
                    .get_mut(ordinary.common.workload_path.as_ref().unwrap())
                    .unwrap();
                *text = text.replace("\"schema_version\": 2", "\"schema_version\": 99");
            }
            _ => unreachable!(),
        }
        let normal_source = Recorded::new(&snapshot);
        let inspect_source = Recorded::new(&snapshot);
        let normal =
            prepare_with_source(&ordinary.common.config_path, &cwd, &normal_source).unwrap_err();
        let inspected =
            inspect_references_with_source(&ordinary.common.config_path, &cwd, &inspect_source)
                .unwrap_err();
        assert_eq!(
            serde_json::to_value(normal).unwrap(),
            serde_json::to_value(inspected).unwrap(),
            "{mutation}"
        );
        assert_eq!(
            *normal_source.calls.borrow(),
            *inspect_source.calls.borrow(),
            "{mutation}"
        );
    }
}
#[test]
fn references_other_valid_profiles_return_explicit_unsupported_without_uses() {
    let cwd = root();
    let i = inspect_references_with_source(
        Path::new("examples/ethernet/unicast.ini"),
        &cwd,
        &FsInputSource,
    )
    .unwrap();
    assert_eq!(i.unsupported.len(), 1);
    assert_eq!(i.unsupported[0].profile, "ethernet.l2.store-forward.v1");
    assert!(!i.declarations.is_empty());
    assert!(i.instances.is_empty() && i.uses.is_empty());
}

#[test]
fn references_qualified_type_tokens_exclude_internal_trivia_for_safe_name_patches() {
    let cwd = root();
    let config = Path::new("examples/gateway/fanout.ini");
    let prepared = prepare_with_source(config, &cwd, &FsInputSource).unwrap();
    let mut snapshot = Snapshot::from_prepared(&prepared);
    for (path, text) in &mut snapshot.files {
        if path.extension().is_some_and(|e| e == "ned") {
            *text = text
                .replace(
                    "gateway.Controller",
                    "gateway /* 日本語 Controller */ . Controller",
                )
                .replace("gateway.Wire", "gateway /* Wire */\n . Wire")
                .replace("src.tx", "src /* src.tx */ . tx");
        }
    }
    let evidence = inspect_references_with_source(config, &cwd, &snapshot).unwrap();
    let mut patches: BTreeMap<String, Vec<SourceSpan>> = BTreeMap::new();
    for u in &evidence.uses {
        if matches!(
            u.syntax_kind,
            ReferenceSyntaxKind::ChildType | ReferenceSyntaxKind::ChannelType
        ) {
            let file = snapshot.files.get(Path::new(&u.span.source)).unwrap();
            assert_eq!(
                u.source_segments
                    .iter()
                    .map(|s| &file[s.start_byte..s.end_byte])
                    .collect::<Vec<_>>()
                    .join("."),
                u.decoded_value
            );
            assert!(
                u.source_segments
                    .iter()
                    .all(|s| !file[s.start_byte..s.end_byte].contains("/*"))
            );
        }
        if u.target == ty("gateway.Controller")
            && matches!(
                u.syntax_kind,
                ReferenceSyntaxKind::ChildType | ReferenceSyntaxKind::TypeDeclaration
            )
        {
            patches
                .entry(u.span.source.clone())
                .or_default()
                .push(u.source_segments.last().unwrap().clone());
        }
    }
    for (path, spans) in &mut patches {
        spans.sort_by_key(|s| std::cmp::Reverse(s.start_byte));
        let text = snapshot.files.get_mut(Path::new(path)).unwrap();
        let before_comments = text.matches("/* 日本語 Controller */").count();
        let before_implementations = text.matches("dir.can.MultibusController").count();
        for span in spans {
            text.replace_range(span.start_byte..span.end_byte, "RenamedController");
        }
        assert_eq!(
            text.matches("/* 日本語 Controller */").count(),
            before_comments
        );
        assert_eq!(
            text.matches("dir.can.MultibusController").count(),
            before_implementations
        );
    }
    let renamed = inspect_references_with_source(config, &cwd, &snapshot).unwrap();
    assert!(
        renamed
            .declarations
            .iter()
            .any(|d| d.qname == "gateway.RenamedController")
    );
    assert_eq!(
        format!("{:?}", evidence.prepared.can),
        format!("{:?}", renamed.prepared.can)
    );
}
