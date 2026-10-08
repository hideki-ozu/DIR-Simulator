use super::*;
use crate::input::{self, Declaration, FsInputSource, InputSource, ned};
use crate::types::{PreparedCan, PreparedCommon, PreparedGateway, PreparedSimulation};
use std::path::Path;

type Result<T> = std::result::Result<T, Diagnostic>;
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(message)
}

/// Freeze a static registry and prepare NED/INI input without constructing models.
/// The default registry includes adapters for every supported built-in profile.
pub fn prepare_with_registry(config: &Path, registry: Registry) -> Result<PreparedSimulation> {
    let cwd = std::env::current_dir().map_err(|e| error(e.to_string()))?;
    prepare_with_registry_and_source(config, &cwd, &FsInputSource, registry)
}
pub fn prepare_with_registry_and_source(
    config: &Path,
    cwd: &Path,
    source: &dyn InputSource,
    registry: Registry,
) -> Result<PreparedSimulation> {
    let config = input::absolute(config, cwd);
    let mut inputs = Vec::new();
    let text = input::read_file(&config, &mut inputs, source)?;
    let header = input::inspect_config(&text, &config, cwd)?;
    let annotate = |key: &str, diagnostic: Diagnostic| {
        input::annotate_config_key(&text, &config, key, diagnostic)
    };
    let annotate_target = |diagnostic: Diagnostic| {
        if let Some(target) = diagnostic.target.clone() {
            if header.general.contains_key(&target)
                || header.channels.contains_key(&target)
                || header
                    .channels
                    .iter()
                    .any(|(id, fields)| fields.keys().any(|key| format!("{id}.{key}") == target))
            {
                return annotate(&target, diagnostic);
            }
        }
        diagnostic
    };
    let profile = header.profile.as_deref().unwrap_or("can.cc.ideal.v1");
    registry.validate(profile).map_err(|diagnostic| {
        if registry.profiles.contains_key(profile) {
            diagnostic
        } else {
            annotate(
                "model-profile",
                diagnostic
                    .with_reason("unsupported_profile")
                    .with_detail("actual", profile)
                    .with_detail("expected", "registered profile"),
            )
        }
    })?;
    if matches!(
        profile,
        "ethernet.l2.dynamic.v1" | "ethernet.tsn.v1" | "can.ethernet.gateway.v1"
    ) {
        let mut prepared = input::prepare_with_source(&config, cwd, source)?;
        prepared
            .registered
            .as_mut()
            .ok_or_else(|| error("missing composed network preparation"))?
            .registry = Arc::new(registry);
        prepared.common.provenance = input::capture_provenance(&prepared)?;
        return Ok(prepared);
    }
    if registry.builtin_profiles.contains(profile) {
        let mut prepared = input::prepare_with_source(&config, cwd, source)?;
        let builtin_node_count = prepared.node_count();
        prepared.registered = Some(PreparedRegistered {
            registry: Arc::new(registry),
            models: Vec::new(),
            channels: Vec::new(),
            adapter: Some(BuiltinAdapter),
            builtin_node_count,
            network: None,
        });
        return Ok(prepared);
    }
    let descriptor = &registry.profiles[profile];
    if descriptor.output_schema_version != 2 {
        return Err(error(
            "registered extension profiles require output schema version 2",
        ));
    }
    let network = header.network.as_deref().ok_or_else(|| {
        annotate(
            "network",
            error("missing network")
                .with_reason("missing_value")
                .with_detail("actual", "missing")
                .with_detail("expected", "network type"),
        )
    })?;
    let mut types = BTreeMap::new();
    for (i, root) in header.roots.iter().enumerate() {
        input::no_symlinks(root, source)?;
        if !source
            .metadata(root)
            .is_ok_and(|m| m.kind == input::InputKind::Directory)
        {
            return Err(
                error(format!("NED root is not a directory: {}", root.display()))
                    .with_reason("input_unreadable")
                    .with_source(root)
                    .with_detail("operation", "metadata")
                    .with_detail("expected", "NED directory"),
            );
        }
        if header.roots[..i]
            .iter()
            .any(|p| root.starts_with(p) || p.starts_with(root))
        {
            return Err(annotate(
                "ned-path",
                error("overlapping NED roots")
                    .with_reason("invalid_argument")
                    .with_detail("actual", root.display())
                    .with_detail("expected", "non-overlapping NED directories"),
            ));
        }
        let mut files = Vec::new();
        input::collect_ned(root, root, &mut files, source)?;
        files.sort();
        for file in files {
            let content = input::read_file(&file, &mut inputs, source)?;
            let package = file
                .strip_prefix(root)
                .unwrap()
                .parent()
                .unwrap()
                .components()
                .map(|p| p.as_os_str().to_str().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(".");
            for declaration in input::parse_ned(&content, &file, &package)?.declarations() {
                if let Some(previous) = types.get(declaration.name()) {
                    let previous: &Declaration = previous;
                    return Err(declaration
                        .name_span()
                        .apply(error(format!("duplicate NED type: {}", declaration.name())))
                        .with_reason("duplicate_definition")
                        .with_target(declaration.name())
                        .with_detail("actual", declaration.name())
                        .with_detail("expected", "unique NED type")
                        .with_detail("related_source", &previous.name_span().source)
                        .with_detail("related_line", previous.name_span().line)
                        .with_detail("related_column", previous.name_span().column));
                }
                types.insert(declaration.name().to_string(), declaration.clone());
            }
        }
    }
    let standard = [
        "network",
        "ned-path",
        "sim-time-limit",
        "metrics-window",
        "max-events",
        "max-delta-cycles",
        "model-profile",
        "model-config",
        "workload",
    ];
    let assignments = header
        .general
        .iter()
        .filter(|(key, _)| !standard.contains(&key.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let rules = Rules {
        registry: &registry,
    };
    let resolved = ned::resolve(&types, network, &assignments, &rules).map_err(annotate_target)?;
    let load_json = |path: &Option<std::path::PathBuf>,
                     inputs: &mut Vec<crate::types::InputSnapshot>|
     -> Result<Option<Value>> {
        path.as_ref()
            .map(|path| {
                let content = input::read_file(path, inputs, source)?;
                input::parse_json(&content).map_err(|diagnostic| diagnostic.with_source(path))
            })
            .transpose()
    };
    let profile_input = Arc::new(ProfileInput {
        model_config: load_json(&header.model_config, &mut inputs)?,
        workload: load_json(&header.workload, &mut inputs)?,
    });
    if let Some(validator) = registry.input_validators.get(profile) {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| validator(&profile_input)))
            .map_err(|_| {
                error("profile input validator panicked")
                    .with_reason("initialize_failed")
                    .with_detail("operation", "validate_profile_input")
            })?
            // ModelError exposes no file, key or span, so callback failures keep input positions null.
            .map_err(|e| {
                error(e.0)
                    .with_reason("model_config_invalid")
                    .with_detail("operation", "validate_profile_input")
            })?;
    }
    let mut models = Vec::new();
    for (id, declaration) in resolved.instances().filter(|(_, d)| d.kind() == "simple") {
        let implementation = declaration.implementation().ok_or_else(|| {
            declaration.span().apply(
                error("simple has no implementation")
                    .with_reason("implementation_missing")
                    .with_target(id),
            )
        })?;
        if !descriptor.modules.iter().any(|m| m == implementation) {
            return Err(declaration.span().apply(
                error(format!("module excluded by profile: {implementation}"))
                    .with_reason("unsupported_profile_setting")
                    .with_target(id)
                    .with_detail("type", implementation)
                    .with_detail("actual", implementation)
                    .with_detail("expected", "module allowed by selected profile"),
            ));
        }
        let module = &registry.modules[implementation].descriptor;
        let mut connections = Vec::new();
        for port in module
            .ports
            .iter()
            .filter(|p| p.direction == Direction::Output)
        {
            let path = resolved.trace(&format!("{id}.{}", port.name))?;
            let (destination, input_port) = path
                .end
                .rsplit_once('.')
                .ok_or_else(|| error("invalid path endpoint"))?;
            let sink = resolved.declaration(destination);
            let sink_module = registry
                .modules
                .get(sink.implementation().unwrap_or(""))
                .ok_or_else(|| error("path does not end at registered simple"))?;
            let sink_port = sink_module
                .descriptor
                .ports
                .iter()
                .find(|p| p.name == input_port && p.direction == Direction::Input)
                .ok_or_else(|| error("invalid sink port"))?;
            if sink_port.schema != port.schema {
                return Err(error(format!(
                    "incompatible payload schema/version: {id}.{}",
                    port.name
                )));
            }
            connections.push(ConnectionDescriptor {
                output_port: port.name.clone(),
                destination: destination.into(),
                input_port: input_port.into(),
                schema: port.schema.clone(),
                channels: path.channel_ids(),
            });
        }
        let overrides: BTreeMap<String, String> = header
            .general
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&format!("{id}."))
                    .filter(|s| !s.contains('.'))
                    .map(|k| (k.to_string(), v.clone()))
            })
            .collect();
        models.push(ModelConfig {
            profile_input: profile_input.clone(),
            id: id.into(),
            subject: id.into(),
            implementation_key: implementation.into(),
            parameters: parameters(declaration, &module.parameters, &overrides, id, &annotate)?,
            output_sources: connections
                .iter()
                .map(|route| (route.output_port.clone(), id.into()))
                .collect(),
            connections,
        });
    }
    let mut channels = Vec::new();
    for (id, declaration) in resolved.channels() {
        let implementation = declaration.implementation().ok_or_else(|| {
            declaration.span().apply(
                error("channel has no implementation")
                    .with_reason("implementation_missing")
                    .with_target(id),
            )
        })?;
        if !descriptor.channels.iter().any(|m| m == implementation) {
            return Err(declaration.span().apply(
                error(format!("channel excluded by profile: {implementation}"))
                    .with_reason("unsupported_profile_setting")
                    .with_target(id)
                    .with_detail("type", implementation)
                    .with_detail("actual", implementation)
                    .with_detail("expected", "channel allowed by selected profile"),
            ));
        }
        let channel = registry
            .channels
            .get(implementation)
            .ok_or_else(|| error("unregistered channel implementation"))?;
        let overrides = header.channels.get(id).cloned().unwrap_or_default();
        channels.push(ChannelConfig {
            id: id.into(),
            implementation_key: implementation.into(),
            parameters: parameters(
                declaration,
                &channel.descriptor.parameters,
                &overrides,
                id,
                &annotate,
            )?,
        });
    }
    for id in header.channels.keys() {
        if !channels.iter().any(|c| &c.id == id) {
            return Err(annotate(
                id,
                error(format!("unknown channel connection: {id}"))
                    .with_reason("invalid_connection")
                    .with_detail("actual", id)
                    .with_detail("expected", "explicit channel connection"),
            ));
        }
    }
    // Profile-owned objects follow all simples in normalized target-path order.
    for ((owner_profile, target), coordinator) in &registry.coordinators {
        if owner_profile != profile {
            continue;
        }
        if !resolved.instances().any(|(id, declaration)| {
            id == target && matches!(declaration.kind(), "module" | "network")
        }) {
            return Err(error(format!(
                "coordinator target is not a compound instance: {target}"
            )));
        }
        if !descriptor.modules.contains(&coordinator.implementation_key) {
            return Err(error(format!(
                "coordinator module excluded by profile: {}",
                coordinator.implementation_key
            )));
        }
        let module = &registry.modules[&coordinator.implementation_key].descriptor;
        validate_coordinator_parameters(&coordinator.parameters, &module.parameters)?;
        let mut connections = Vec::new();
        let mut output_sources = BTreeMap::new();
        for output in &coordinator.outputs {
            if !output.source.starts_with(&format!("{target}.")) {
                return Err(error(format!(
                    "coordinator output outside target: {}",
                    output.source
                )));
            }
            let source = models
                .iter()
                .find(|model| model.id == output.source && model.id == model.subject)
                .ok_or_else(|| {
                    error(format!(
                        "coordinator output source is not a simple instance: {}",
                        output.source
                    ))
                })?;
            let route = source
                .connections
                .iter()
                .find(|route| route.output_port == output.output_port)
                .ok_or_else(|| {
                    error(format!(
                        "unknown coordinator borrowed output: {}.{}",
                        output.source, output.output_port
                    ))
                })?;
            let port = module
                .ports
                .iter()
                .find(|port| port.name == output.handle && port.direction == Direction::Output)
                .ok_or_else(|| {
                    error(format!(
                        "undeclared coordinator output handle: {}",
                        output.handle
                    ))
                })?;
            if route.schema != port.schema {
                return Err(error("coordinator borrowed output schema/version mismatch"));
            }
            let mut binding = route.clone();
            binding.output_port = output.handle.clone();
            connections.push(binding);
            output_sources.insert(output.handle.clone(), source.id.clone());
        }
        if module
            .ports
            .iter()
            .filter(|port| port.direction == Direction::Output)
            .count()
            != connections.len()
        {
            return Err(error("coordinator has unbound output handles"));
        }
        connections.sort_by(|a, b| a.output_port.cmp(&b.output_port));
        models.push(ModelConfig {
            profile_input: profile_input.clone(),
            id: format!("@profile:{profile}:{target}"),
            subject: target.clone(),
            implementation_key: coordinator.implementation_key.clone(),
            parameters: coordinator.parameters.clone(),
            connections,
            output_sources,
        });
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (descriptor.validate)(&models, &channels)
    }))
    .map_err(|_| {
        error("profile validator panicked")
            .with_reason("initialize_failed")
            .with_detail("operation", "validate_profile")
    })?
    .map_err(|e| {
        error(e.0)
            .with_reason("model_config_invalid")
            .with_detail("operation", "validate_profile")
    })?;
    let required = |key: &str| {
        header.general.get(key).map(String::as_str).ok_or_else(|| {
            annotate(
                key,
                error(format!("missing {key}"))
                    .with_reason("missing_value")
                    .with_detail("actual", "missing")
                    .with_detail("expected", "General setting"),
            )
        })
    };
    let time_limit_ps = input::parse_time(required("sim-time-limit")?)
        .map_err(|diagnostic| annotate("sim-time-limit", diagnostic))?;
    let metrics_window_ps = input::parse_time(
        header
            .general
            .get("metrics-window")
            .map(String::as_str)
            .unwrap_or("1ms"),
    )
    .map_err(|diagnostic| annotate("metrics-window", diagnostic))?;
    if metrics_window_ps == 0 {
        return Err(annotate(
            "metrics-window",
            error("metrics-window must be positive")
                .with_reason("invalid_range")
                .with_detail("actual", "0")
                .with_detail("expected", "positive duration"),
        ));
    }
    let limit = |key: &str, default: u64| -> Result<u64> {
        let Some(value) = header.general.get(key) else {
            return Ok(default);
        };
        input::unsigned(value, true).map_err(|diagnostic| annotate(key, diagnostic))
    };
    let common = PreparedCommon {
        profile: profile.into(),
        network: network.into(),
        module_paths: resolved.module_paths(),
        time_limit_ps,
        metrics_window_ps,
        max_events: limit("max-events", 100_000_000)?,
        max_delta_cycles: limit("max-delta-cycles", 1_000_000)?,
        channel_count: channels.len(),
        config_path: config.clone(),
        model_config_path: header.model_config,
        workload_path: header.workload,
        inputs,
        provenance: Default::default(),
        run_identity: None,
    };
    let mut prepared = PreparedSimulation {
        common,
        can: PreparedCan {
            bus_id: String::new(),
            bitrate: 0,
            buses: Vec::new(),
            controller_buses: Vec::new(),
            controllers: Vec::new(),
            generators: Vec::new(),
        },
        gateway: PreparedGateway {
            gateways: Vec::new(),
            controller_gateways: Vec::new(),
        },
        ethernet: None,
        canfd: None,
        axi: None,
        soc: None,
        memory_ipc: None,
        registered: Some(PreparedRegistered {
            registry: Arc::new(registry),
            models,
            channels,
            adapter: None,
            builtin_node_count: 0,
            network: None,
        }),
    };
    prepared.common.provenance = input::capture_provenance(&prepared)?;
    Ok(prepared)
}

struct Rules<'a> {
    registry: &'a Registry,
}
fn validate_coordinator_parameters(
    values: &Parameters,
    descriptors: &[ParameterDescriptor],
) -> Result<()> {
    for name in values.keys() {
        if !descriptors
            .iter()
            .any(|descriptor| &descriptor.name == name)
        {
            return Err(error(format!("unknown coordinator parameter: {name}")));
        }
    }
    for descriptor in descriptors {
        let Some(value) = values.get(&descriptor.name) else {
            if descriptor.required {
                return Err(error(format!(
                    "missing coordinator parameter: {}",
                    descriptor.name
                )));
            }
            continue;
        };
        let valid_type = match (descriptor.ned_type.as_str(), descriptor.dimension, value) {
            ("int", Dimension::Dimensionless, ParameterValue::Integer(_))
            | ("double", Dimension::Dimensionless, ParameterValue::Double(_))
            | ("bool", Dimension::Dimensionless, ParameterValue::Boolean(_))
            | ("string", Dimension::Dimensionless, ParameterValue::String(_)) => true,
            ("int" | "double", dimension, ParameterValue::Quantity(_)) => {
                dimension != Dimension::Dimensionless
            }
            _ => false,
        };
        let numeric = match value {
            ParameterValue::Integer(value) => Some(*value as f64),
            ParameterValue::Quantity(value) => Some(*value as f64),
            ParameterValue::Double(value) => Some(*value),
            _ => None,
        };
        if !valid_type
            || numeric.is_some_and(|value| {
                !value.is_finite()
                    || descriptor.minimum.is_some_and(|min| value < min)
                    || descriptor.maximum.is_some_and(|max| value > max)
            })
        {
            return Err(error(format!(
                "invalid coordinator parameter type or bounds: {}",
                descriptor.name
            )));
        }
    }
    Ok(())
}
impl ned::ModelRules for Rules<'_> {
    fn validate_schema(&self, declaration: &Declaration) -> Result<()> {
        let result = (|| {
            let key = declaration.implementation().unwrap_or("");
            let parameters = if declaration.kind() == "simple" {
                let module = self.registry.modules.get(key).ok_or_else(|| {
                    error(format!("unregistered module: {key}"))
                        .with_reason("implementation_missing")
                        .with_detail("type", key)
                })?;
                let actual: BTreeMap<_, _> = module
                    .descriptor
                    .ports
                    .iter()
                    .map(|p| (p.name.clone(), p.direction == Direction::Output))
                    .collect();
                if &actual != declaration.gates() {
                    return Err(
                        error(format!("port schema mismatch for {}", declaration.name()))
                            .with_reason("invalid_connection"),
                    );
                }
                &module.descriptor.parameters
            } else if declaration.kind() == "channel" {
                &self
                    .registry
                    .channels
                    .get(key)
                    .ok_or_else(|| {
                        error(format!("unregistered channel: {key}"))
                            .with_reason("implementation_missing")
                            .with_detail("type", key)
                    })?
                    .descriptor
                    .parameters
            } else {
                return Err(error(format!("unregistered implementation: {key}"))
                    .with_reason("implementation_missing")
                    .with_detail("type", key));
            };
            check_parameters(declaration, parameters)
        })();
        result.map_err(|diagnostic| {
            if diagnostic.source.is_some() {
                diagnostic
            } else {
                declaration
                    .span()
                    .apply(diagnostic)
                    .with_target(declaration.name())
            }
        })
    }
    fn validate_value(
        &self,
        declaration: &Declaration,
        name: &str,
        value: &ned::TypedValue,
    ) -> Result<()> {
        let implementation = declaration.implementation().unwrap_or("");
        let descriptors = self
            .registry
            .modules
            .get(implementation)
            .map(|module| &module.descriptor.parameters)
            .or_else(|| {
                self.registry
                    .channels
                    .get(implementation)
                    .map(|channel| &channel.descriptor.parameters)
            });
        let Some(descriptor) = descriptors
            .and_then(|parameters| parameters.iter().find(|parameter| parameter.name == name))
        else {
            return Ok(());
        };
        let numeric = match value {
            ned::TypedValue::Integer(value) => Some(*value as f64),
            ned::TypedValue::Quantity(value) => Some(*value as f64),
            // The shared typed enum validates finite doubles but does not retain their value.
            // Defaults are checked independently of instances; adopted overrides are checked by parameters().
            ned::TypedValue::Double => declaration.parameters()[name]
                .default()
                .and_then(|literal| literal.parse::<f64>().ok()),
            _ => None,
        };
        if numeric.is_some_and(|value| {
            descriptor.minimum.is_some_and(|minimum| value < minimum)
                || descriptor.maximum.is_some_and(|maximum| value > maximum)
        }) {
            return Err(
                error(format!("parameter outside registered bounds: {name}"))
                    .with_reason("invalid_range")
                    .with_target(format!("{}.{}", declaration.name(), name))
                    .with_detail("actual", numeric.unwrap())
                    .with_detail(
                        "expected",
                        format!(
                            "minimum={:?}, maximum={:?}",
                            descriptor.minimum, descriptor.maximum
                        ),
                    ),
            );
        }
        Ok(())
    }
    // Full versioned payload checks are performed after resolution without leaking strings.
    fn payload(&self, _declaration: &Declaration, _gate: &str) -> Option<&'static str> {
        None
    }
}
fn check_parameters(declaration: &Declaration, descriptors: &[ParameterDescriptor]) -> Result<()> {
    if declaration.parameters().len() != descriptors.len() {
        return Err(declaration.span().apply(
            error(format!("parameter schema mismatch: {}", declaration.name()))
                .with_reason("invalid_type")
                .with_target(declaration.name()),
        ));
    }
    for descriptor in descriptors {
        let p = declaration
            .parameters()
            .get(&descriptor.name)
            .ok_or_else(|| {
                declaration.span().apply(
                    error(format!(
                        "missing parameter declaration: {}",
                        descriptor.name
                    ))
                    .with_reason("missing_value")
                    .with_target(format!(
                        "{}.{}",
                        declaration.name(),
                        descriptor.name
                    )),
                )
            })?;
        let unit = match descriptor.dimension {
            Dimension::Dimensionless => None,
            Dimension::Time => Some("s"),
            Dimension::Bitrate => Some("bps"),
            Dimension::Data => Some("B"),
        };
        if p.scalar() != descriptor.ned_type || p.unit() != unit {
            return Err(p.span().apply(
                error(format!(
                    "parameter type/dimension mismatch: {}",
                    descriptor.name
                ))
                .with_reason("invalid_type")
                .with_target(format!("{}.{}", declaration.name(), descriptor.name))
                .with_detail("actual", format!("{} {:?}", p.scalar(), p.unit()))
                .with_detail("expected", format!("{} {:?}", descriptor.ned_type, unit)),
            ));
        }
    }
    Ok(())
}
fn parameters(
    declaration: &Declaration,
    descriptors: &[ParameterDescriptor],
    overrides: &BTreeMap<String, String>,
    target: &str,
    annotate: &impl Fn(&str, Diagnostic) -> Diagnostic,
) -> Result<Parameters> {
    check_parameters(declaration, descriptors)?;
    for key in overrides.keys() {
        if !declaration.parameters().contains_key(key) {
            let full_key = format!("{target}.{key}");
            return Err(annotate(
                &full_key,
                error(format!("unknown parameter override: {key}"))
                    .with_reason("unknown_parameter")
                    .with_detail("actual", key)
                    .with_detail("expected", "registered parameter"),
            ));
        }
    }
    let mut values = BTreeMap::new();
    for d in descriptors {
        let p = &declaration.parameters()[&d.name];
        let full_key = format!("{target}.{}", d.name);
        let locate = |diagnostic: Diagnostic| {
            let diagnostic = diagnostic.with_target(&full_key);
            if overrides.contains_key(&d.name) {
                annotate(&full_key, diagnostic)
            } else {
                p.default_span()
                    .unwrap_or_else(|| p.span())
                    .apply(diagnostic)
            }
        };
        let literal = overrides
            .get(&d.name)
            .map(String::as_str)
            .or_else(|| p.default())
            .ok_or_else(|| {
                locate(
                    error(format!("parameter has no value: {}", d.name))
                        .with_reason("missing_value")
                        .with_detail("actual", "missing")
                        .with_detail("expected", &d.ned_type),
                )
            })?;
        let typed = ned::typed_value(p, literal).map_err(|diagnostic| {
            locate(
                diagnostic
                    .with_detail("actual", literal)
                    .with_detail("expected", &d.ned_type),
            )
        })?;
        let value = match typed {
            ned::TypedValue::Integer(v) => ParameterValue::Integer(v),
            ned::TypedValue::Quantity(v) => ParameterValue::Quantity(v),
            ned::TypedValue::Double => ParameterValue::Double(literal.parse().map_err(|_| {
                locate(
                    error("invalid double")
                        .with_reason("invalid_type")
                        .with_detail("actual", literal)
                        .with_detail("expected", "finite double"),
                )
            })?),
            ned::TypedValue::Boolean => ParameterValue::Boolean(literal == "true"),
            ned::TypedValue::String(v) => ParameterValue::String(v),
        };
        let numeric = match value {
            ParameterValue::Integer(v) => Some(v as f64),
            ParameterValue::Quantity(v) => Some(v as f64),
            ParameterValue::Double(v) => Some(v),
            _ => None,
        };
        if numeric.is_some_and(|v| {
            d.minimum.is_some_and(|min| v < min) || d.maximum.is_some_and(|max| v > max)
        }) {
            return Err(locate(
                error(format!("parameter outside registered bounds: {}", d.name))
                    .with_reason("invalid_range")
                    .with_detail("actual", literal)
                    .with_detail(
                        "expected",
                        format!("minimum={:?}, maximum={:?}", d.minimum, d.maximum),
                    ),
            ));
        }
        values.insert(d.name.clone(), value);
    }
    Ok(values)
}
