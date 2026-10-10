//! Reproduction data captured solely from validated, owned input snapshots.
use super::{Result, inspect_config, ned};
use crate::types::{PreparedSimulation, SourceSpan};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct RunIdentity {
    pub run_id: String,
    pub started_at_utc: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceRoot {
    pub logical_root: String,
    pub canonical_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct PreparedProvenance {
    pub roots: Vec<SourceRoot>,
    pub declarations: Vec<Value>,
    pub values: Vec<Value>,
    pub channels: Vec<Value>,
    pub resolved_config: BTreeMap<String, String>,
}

impl PreparedProvenance {
    pub fn logical_path(&self, prepared: &PreparedSimulation, path: &Path) -> String {
        if path == prepared.common.config_path {
            return "config".into();
        }
        if Some(path) == prepared.common.model_config_path.as_deref() {
            return "model-config".into();
        }
        if Some(path) == prepared.common.workload_path.as_deref() {
            return "workload".into();
        }
        for root in &self.roots {
            if let Ok(relative) = path.strip_prefix(&root.canonical_path) {
                return format!(
                    "{}/{}",
                    root.logical_root,
                    relative.to_string_lossy().replace('\\', "/")
                );
            }
        }
        // Hand-built PreparedSimulation values need not contain an INI snapshot.
        format!(
            "unmapped/{}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    }
}

fn canonical(value: &Value) -> String {
    // serde_json maps use sorted keys unless preserve_order is explicitly enabled.
    serde_json::to_string(value).expect("finite validated reproduction value")
}

fn normalized(parameter: &ned::Parameter, literal: &str) -> Result<String> {
    let value = ned::typed_value(parameter, literal)?;
    Ok(match value {
        ned::TypedValue::Integer(n) => n.to_string(),
        ned::TypedValue::Quantity(n) => format!(
            "{n}{}",
            match parameter.unit() {
                Some("s") => "ps",
                Some("bps") => "bit/s",
                Some("B") => "byte",
                _ => unreachable!(),
            }
        ),
        ned::TypedValue::String(s) => canonical(&json!(s)),
        ned::TypedValue::Boolean => literal.into(),
        ned::TypedValue::Double => {
            canonical(&json!(literal.parse::<f64>().expect("validated double")))
        }
    })
}

fn span_value(
    span: &SourceSpan,
    prepared: &PreparedSimulation,
    provenance: &PreparedProvenance,
) -> Value {
    let mut value = json!(span);
    value["source"] = json!(provenance.logical_path(prepared, Path::new(&span.source)));
    value
}

/// The INI parser has already validated syntax. Record value token positions
/// without reopening files or attempting another configuration interpretation.
fn ini_locations(text: &str) -> BTreeMap<String, SourceSpan> {
    let mut spans = BTreeMap::new();
    let mut section = String::new();
    let mut offset = 0;
    for (line, raw) in text.split_inclusive('\n').enumerate() {
        let stripped = raw.trim_start_matches('\u{feff}').trim();
        if stripped.starts_with('[') {
            section = stripped
                .strip_prefix("[Channel ")
                .and_then(|s| s.strip_suffix(']'))
                .unwrap_or("")
                .into();
        } else if !stripped.starts_with([';', '#']) {
            if let Some((key, value)) = stripped.split_once('=') {
                let value = value.trim();
                let equal = raw.find('=').unwrap();
                let start = equal + 1 + raw[equal + 1..].len()
                    - raw[equal + 1..].trim_start_matches([' ', '\t']).len();
                let key = if section.is_empty() {
                    key.trim().into()
                } else {
                    format!("{section}.{}", key.trim())
                };
                let column = raw[..start].trim_start_matches('\u{feff}').chars().count() + 1;
                spans.insert(
                    key,
                    SourceSpan {
                        source: "config".into(),
                        line: line + 1,
                        column,
                        end_line: line + 1,
                        end_column: column + value.chars().count(),
                        start_byte: offset + start,
                        end_byte: offset + start + value.len(),
                    },
                );
            }
        }
        offset += raw.len();
    }
    spans
}

pub fn capture_provenance(prepared: &PreparedSimulation) -> Result<PreparedProvenance> {
    let mut p = PreparedProvenance::default();
    let Some(config) = prepared
        .common
        .inputs
        .iter()
        .find(|s| s.path == prepared.common.config_path)
    else {
        return Ok(p);
    };
    let header = inspect_config(
        &config.content,
        &config.path,
        config.path.parent().unwrap_or(Path::new("/")),
    )?;
    p.roots = header
        .roots
        .iter()
        .enumerate()
        .map(|(i, path)| SourceRoot {
            logical_root: format!("root{i}"),
            canonical_path: path.clone(),
        })
        .collect();
    let ini_spans = ini_locations(&config.content);
    let mut declarations = BTreeMap::new();
    for input in &prepared.common.inputs {
        if input.path.extension().is_none_or(|ext| ext != "ned") {
            continue;
        }
        let Some(root) = p
            .roots
            .iter()
            .find(|r| input.path.starts_with(&r.canonical_path))
        else {
            continue;
        };
        let relative = input.path.strip_prefix(&root.canonical_path).unwrap();
        let package = relative
            .parent()
            .unwrap_or(Path::new(""))
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join(".");
        for d in ned::parse(&input.content, &input.path, &package)? {
            p.declarations.push(json!({"type":d.name(),"kind":d.kind(),"span":span_value(d.span(),prepared,&p),"parameters":d.parameters().iter().map(|(name,v)|json!({"name":name,"declared_type":v.scalar(),"unit":v.unit(),"declaration_span":span_value(v.span(),prepared,&p),"default_span":v.default_span().map(|span|span_value(span,prepared,&p)),"normalized_default":v.default().map(|value|normalized(v,value)).transpose().unwrap()})).collect::<Vec<_>>() }));
            declarations.insert(d.name().into(), d);
        }
    }
    fn expand(
        instance: &str,
        type_name: &str,
        declarations: &BTreeMap<String, ned::Declaration>,
        header: &super::ProjectHeader,
        ini_spans: &BTreeMap<String, SourceSpan>,
        prepared: &PreparedSimulation,
        p: &mut PreparedProvenance,
    ) -> Result<()> {
        let d = &declarations[type_name];
        for (name, parameter) in d.parameters() {
            let key = format!("{instance}.{name}");
            let value = header
                .general
                .get(&key)
                .map(String::as_str)
                .or(parameter.default());
            if let Some(value) = value {
                let normalized = normalized(parameter, value)?;
                let ini = header.general.contains_key(&key);
                p.resolved_config.insert(key.clone(), normalized.clone());
                p.values.push(json!({"key":key,"declared_type":parameter.scalar(),"physical_quantity":parameter.unit(),"normalized_value":normalized,"adopted_source":if ini {"INI"} else {"default"},"adopted_span":if ini {json!(ini_spans.get(&key))} else {parameter.default_span().map(|s|span_value(s,prepared,p)).unwrap_or(Value::Null)},"declaration_span":span_value(parameter.span(),prepared,p),"default_span":parameter.default_span().map(|s|span_value(s,prepared,p))}));
            }
        }
        for connection in d.connections() {
            let id = format!("{instance}::{}", connection.start());
            let mut settings = BTreeMap::new();
            if let Some(channel) = connection.channel() {
                let channel_decl = &declarations[channel];
                for (name, param) in channel_decl.parameters() {
                    let key = format!("{id}.{name}");
                    let override_value =
                        header.channels.get(&id).and_then(|values| values.get(name));
                    if let Some(literal) = override_value.map(String::as_str).or(param.default()) {
                        let value = normalized(param, literal)?;
                        settings.insert(name.clone(), value.clone());
                        p.resolved_config.insert(key.clone(), value.clone());
                        p.values.push(json!({"key":key,"declared_type":param.scalar(),"physical_quantity":param.unit(),"normalized_value":value,"adopted_source":if override_value.is_some(){"INI"}else{"default"},"adopted_span":if override_value.is_some(){json!(ini_spans.get(&key))}else{param.default_span().map(|s|span_value(s,prepared,p)).unwrap_or(Value::Null)},"declaration_span":span_value(param.span(),prepared,p),"default_span":param.default_span().map(|s|span_value(s,prepared,p))}));
                    }
                }
            }
            p.channels.push(json!({"instance":id,"state":canonical(&json!({"type":connection.channel(),"source":format!("{instance}.{}",connection.start()),"destination":format!("{instance}.{}",connection.end()),"pending":[],"settings":settings}))}));
        }
        for (name, child_type) in d.children() {
            expand(
                &format!("{instance}.{name}"),
                child_type,
                declarations,
                header,
                ini_spans,
                prepared,
                p,
            )?;
        }
        Ok(())
    }
    if declarations.contains_key(&prepared.common.network) {
        let root = prepared.common.network.rsplit('.').next().unwrap();
        expand(
            root,
            &prepared.common.network,
            &declarations,
            &header,
            &ini_spans,
            prepared,
            &mut p,
        )?;
    }
    for (key, value, input_key) in [
        (
            "network",
            canonical(&json!(prepared.common.network)),
            "network",
        ),
        (
            "time-limit",
            format!("{}ps", prepared.common.time_limit_ps),
            "sim-time-limit",
        ),
        (
            "metrics-window",
            format!("{}ps", prepared.common.metrics_window_ps),
            "metrics-window",
        ),
        (
            "max-events",
            prepared.common.max_events.to_string(),
            "max-events",
        ),
        (
            "max-delta-cycles",
            prepared.common.max_delta_cycles.to_string(),
            "max-delta-cycles",
        ),
        (
            "model-profile",
            canonical(&json!(prepared.common.profile)),
            "model-profile",
        ),
    ] {
        p.resolved_config.insert(key.into(), value.clone());
        p.values.push(json!({"key":key,"normalized_value":value,"adopted_source":if ini_spans.contains_key(input_key){"INI"}else{"runtime-default"},"adopted_span":ini_spans.get(input_key),"declaration_span":null,"default_span":null}));
    }
    p.declarations
        .sort_by(|a, b| a["type"].as_str().cmp(&b["type"].as_str()));
    p.values
        .sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    p.channels
        .sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    Ok(p)
}

/// Ledger for the adapter's actual effective values. JSON source positions are
/// structural (object identity plus field), never inferred by matching literals.
/// Calculated values and defaults remain explicitly distinguishable from input.
pub(crate) fn effective_value_provenance(
    prepared: &PreparedSimulation,
    effective: &Value,
) -> Result<Vec<Value>> {
    use super::json_diagnostics::JsonDocument;
    let documents = prepared
        .common
        .inputs
        .iter()
        .filter(|input| {
            Some(&input.path) == prepared.common.model_config_path.as_ref()
                || Some(&input.path) == prepared.common.workload_path.as_ref()
        })
        .map(|input| Ok((input, JsonDocument::parse(&input.content)?)))
        .collect::<Result<Vec<_>>>()?;
    fn escape(s: &str) -> String {
        s.replace('~', "~0").replace('/', "~1")
    }
    fn identities(value: &Value, pointer: &str, out: &mut BTreeMap<String, String>) {
        if let Some(object) = value.as_object() {
            for key in ["node", "id", "port"] {
                if let Some(id) = object.get(key).and_then(Value::as_str) {
                    out.entry(id.into()).or_insert_with(|| pointer.into());
                }
            }
            for (key, value) in object {
                identities(value, &format!("{pointer}/{}", escape(key)), out);
            }
        } else if let Some(array) = value.as_array() {
            for (i, value) in array.iter().enumerate() {
                identities(value, &format!("{pointer}/{i}"), out);
            }
        }
    }
    let indexes = documents
        .iter()
        .map(|(_, document)| {
            let mut index = BTreeMap::new();
            identities(&document.value, "", &mut index);
            index
        })
        .collect::<Vec<_>>();
    struct Collector<'a, 'b> {
        prepared: &'a PreparedSimulation,
        documents: &'a [(&'a crate::types::InputSnapshot, JsonDocument<'b>)],
        indexes: &'a [BTreeMap<String, String>],
        rows: Vec<Value>,
    }
    impl Collector<'_, '_> {
        fn walk(&mut self, value: &Value, pointer: &str, identity: Option<&str>) {
            if let Some(object) = value.as_object() {
                let identity = ["node", "id", "port"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(Value::as_str))
                    .or(identity);
                for (key, value) in object {
                    self.walk(value, &format!("{pointer}/{}", escape(key)), identity);
                }
            } else if let Some(array) = value.as_array() {
                // Empty arrays are resolved values too (e.g. no faults).
                if array.is_empty() {
                    self.leaf(value, pointer, identity);
                } else {
                    for (i, value) in array.iter().enumerate() {
                        self.walk(value, &format!("{pointer}/{i}"), identity);
                    }
                }
            } else {
                self.leaf(value, pointer, identity);
            }
        }
        fn leaf(&mut self, value: &Value, pointer: &str, identity: Option<&str>) {
            let field = pointer.rsplit('/').next().unwrap();
            let alias = match field {
                "id" => "node",
                "clock_period_ps" => "clock_period",
                "tx_processing_delay_ps" | "tx_processing_ps" => "tx_processing_delay",
                "rx_processing_delay_ps" | "rx_processing_ps" => "rx_processing_delay",
                "forward_delay_ps" => "forward_delay",
                "faults" => "fault_ranges",
                other => other,
            };
            let mut sources = Vec::new();
            let mut direct = false;
            for (index, (input, document)) in self.documents.iter().enumerate() {
                let base = identity
                    .and_then(|id| self.indexes[index].get(id))
                    .map(String::as_str)
                    .unwrap_or("");
                let candidate = if identity.is_some() && !base.is_empty() {
                    format!("{base}/{}", escape(alias))
                } else if pointer.matches('/').count() == 1 {
                    format!("/{}", escape(alias))
                } else {
                    pointer.into()
                };
                let adopted = if document.value.pointer(&candidate).is_some() {
                    direct = true;
                    candidate.as_str()
                } else {
                    base
                };
                if let Some(span) = document.span(&input.path, adopted) {
                    sources.push(json!({"source_pointer":adopted,"span":span_value(&span,self.prepared,&self.prepared.common.provenance)}));
                }
            }
            let ned_key = identity.map(|id| {
                format!(
                    "{id}.{}",
                    match field {
                        "queue_capacity" => "queueCapacity",
                        "tx_processing_delay_ps" | "tx_processing_ps" => "txProcessingDelay",
                        "rx_processing_delay_ps" | "rx_processing_ps" => "rxProcessingDelay",
                        "rx_filter" => "rxFilter",
                        "bitrate" => "bitrate",
                        other => other,
                    }
                )
            });
            let ned_value = ned_key.as_deref().and_then(|key| {
                self.prepared
                    .common
                    .provenance
                    .values
                    .iter()
                    .find(|v| v["key"] == key)
            });
            if let Some(ned) = ned_value {
                sources.push(json!({"source_key":ned["key"],"span":ned["adopted_span"]}));
                direct = true;
            }
            self.rows.push(json!({"key":format!("@prepared-model{pointer}"),"normalized_value":canonical(value),"adopted_source":if direct{"adapter-resolved-input"}else{"adapter-default-or-derived"},"source_dependencies":sources,"default_definition":"build_source_sha256"}));
        }
    }
    let mut collector = Collector {
        prepared,
        documents: &documents,
        indexes: &indexes,
        rows: Vec::new(),
    };
    collector.walk(effective, "", None);
    collector
        .rows
        .sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    Ok(collector.rows)
}
