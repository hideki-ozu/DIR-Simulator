//! Composed profile preparation using the existing strict VLAN decoder.
use super::{Result, error};
use crate::types::{ethernet::PreparedEthernet, network::PreparedNetwork};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
pub(crate) mod topology;

fn exact(value: &Value, fields: &[&str], label: &str) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| error(format!("{label} must be object")))?;
    if object.len() != fields.len() || fields.iter().any(|key| !object.contains_key(*key)) {
        return Err(error(format!(
            "{label} requires exactly {}",
            fields.join(", ")
        )));
    }
    Ok(())
}
pub(crate) fn prepare(
    mut ethernet: PreparedEthernet,
    config: Value,
    workload: Value,
    profile: &str,
    config_source: (&str, &Path),
    workload_source: Option<(&str, &Path)>,
) -> Result<PreparedNetwork> {
    let config_document = super::JsonDocument::parse(config_source.0)?;
    let workload_document = workload_source
        .map(|(text, _)| super::JsonDocument::parse(text))
        .transpose()?;
    let base_prefix = if profile == "ethernet.tsn.v1" {
        "/base/config"
    } else {
        ""
    };
    let config_error = |diagnostic| annotate(&config_document, config_source.1, "", diagnostic);
    let base_error =
        |diagnostic| annotate(&config_document, config_source.1, base_prefix, diagnostic);
    let workload_error = |diagnostic| {
        if let Some((_, path)) = workload_source {
            annotate(workload_document.as_ref().unwrap(), path, "", diagnostic)
        } else {
            diagnostic
        }
    };
    let base = if profile == "ethernet.tsn.v1" {
        exact(
            &config,
            &["schema_version", "base", "tsn"],
            "TSN configuration",
        )
        .map_err(config_error)?;
        if config["schema_version"].as_u64() != Some(1) {
            return Err(config_error(
                error("TSN schema_version must be integer 1").with_target("/schema_version"),
            ));
        }
        exact(&config["base"], &["profile", "config"], "TSN base")
            .map_err(|d| annotate(&config_document, config_source.1, "/base", d))?;
        if config["base"]["profile"] != "ethernet.l2.dynamic.v1" {
            return Err(config_error(
                error("TSN base profile must be ethernet.l2.dynamic.v1")
                    .with_target("/base/profile"),
            ));
        }
        &config["base"]["config"]
    } else {
        &config
    };
    exact(
        base,
        &[
            "schema_version",
            "endpoints",
            "switches",
            "ports",
            "outputs",
            "dynamic",
        ],
        "dynamic configuration",
    )
    .map_err(base_error)?;
    if base["schema_version"].as_u64() != Some(4) {
        return Err(base_error(
            error("dynamic schema_version must be integer 4").with_target("/schema_version"),
        ));
    }
    exact(
        &workload,
        &["schema_version", "generators", "controls"],
        "dynamic workload",
    )
    .map_err(workload_error)?;
    if workload["schema_version"].as_u64() != Some(4) {
        return Err(workload_error(
            error("dynamic workload schema_version must be integer 4")
                .with_target("/schema_version"),
        ));
    }
    let mut static_config = base.clone();
    static_config.as_object_mut().unwrap().remove("dynamic");
    static_config["schema_version"] = json!(3);
    // Decode references against declared capability, then restore the static set.
    // Registration controls alone determine effective runtime membership.
    let original_vlans =
        add_registrable_capabilities(&mut static_config, &base["dynamic"]).map_err(base_error)?;
    super::ethernet::configure(
        &static_config.to_string(),
        &mut ethernet,
        "ethernet.l2.vlan.v1",
    )
    .map_err(base_error)?;
    let mut static_workload = workload.clone();
    static_workload.as_object_mut().unwrap().remove("controls");
    static_workload["schema_version"] = json!(3);
    let generators = static_workload["generators"]
        .as_array_mut()
        .ok_or_else(|| {
            workload_error(error("generators must be array").with_target("/generators"))
        })?;
    for (index, generator) in generators.iter_mut().enumerate() {
        let pointer = format!("/generators/{index}/frame");
        let frame = generator
            .get_mut("frame")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                workload_error(error("generator requires frame object").with_target(&pointer))
            })?;
        if frame.remove("ip_multicast").is_none() {
            return Err(workload_error(
                error("dynamic frame requires nullable ip_multicast")
                    .with_target(format!("{pointer}/ip_multicast")),
            ));
        }
    }
    super::ethernet::workload(
        &static_workload.to_string(),
        &mut ethernet,
        "ethernet.l2.vlan.v1",
    )
    .map_err(workload_error)?;
    for policy in &mut ethernet.port_policies {
        policy
            .vlans
            .retain(|vid, _| original_vlans[&policy.port].contains(vid));
    }
    let dynamic = super::ethernet::dynamic::prepare(&base["dynamic"], &workload, &ethernet)
        .map_err(|diagnostic| {
            if diagnostic
                .target
                .as_deref()
                .is_some_and(|target| target.starts_with("workload"))
            {
                workload_error(diagnostic)
            } else {
                base_error(diagnostic)
            }
        })?;
    let tsn = if profile == "ethernet.tsn.v1" {
        Some(super::ethernet::tsn::prepare(&config, &ethernet).map_err(config_error)?)
    } else {
        None
    };
    Ok(PreparedNetwork {
        ethernet,
        dynamic: Some(dynamic),
        tsn,
        bridge: None,
        config,
        workload,
    })
}

fn annotate(
    document: &super::JsonDocument<'_>,
    path: &Path,
    prefix: &str,
    diagnostic: crate::Diagnostic,
) -> crate::Diagnostic {
    let target = diagnostic.target.as_deref().unwrap_or("");
    let pointer = if target.starts_with('/') {
        target.to_owned()
    } else {
        let target = target.strip_prefix("workload.").unwrap_or(target);
        let parts = target.replace('[', ".").replace(']', "");
        parts
            .split('.')
            .filter(|part| !part.is_empty())
            .fold(String::new(), |mut pointer, part| {
                pointer.push('/');
                pointer.push_str(&part.replace('~', "~0").replace('/', "~1"));
                pointer
            })
    };
    document
        .annotate(&format!("{prefix}{pointer}"), diagnostic)
        .with_source(path)
}

fn add_registrable_capabilities(
    config: &mut Value,
    dynamic: &Value,
) -> Result<BTreeMap<String, Vec<u16>>> {
    let ports = config["ports"]
        .as_array_mut()
        .ok_or_else(|| error("ports must be array").with_target("/ports"))?;
    let mut original = BTreeMap::new();
    for (index, row) in ports.iter_mut().enumerate() {
        let port = row["port"]
            .as_str()
            .ok_or_else(|| {
                error("port must be string").with_target(format!("/ports/{index}/port"))
            })?
            .to_owned();
        let pvid = row["pvid"].as_u64();
        let vlans = row["vlans"].as_array_mut().ok_or_else(|| {
            error("vlans must be array").with_target(format!("/ports/{index}/vlans"))
        })?;
        if !vlans
            .iter()
            .any(|vlan| vlan["vid"].as_u64() == pvid && pvid.is_some())
        {
            return Err(error("PVID must be a static VLAN member")
                .with_target(format!("/ports/{index}/pvid")));
        }
        original.insert(
            port.clone(),
            vlans
                .iter()
                .filter_map(|vlan| vlan["vid"].as_u64().and_then(|vid| u16::try_from(vid).ok()))
                .collect(),
        );
        if let Some(registrable) = dynamic["registrable"].as_array() {
            for capability in registrable
                .iter()
                .filter(|value| value["port"] == port && value["tagged"] == true)
            {
                if !vlans.iter().any(|vlan| vlan["vid"] == capability["vid"]) {
                    vlans.push(json!({"vid":capability["vid"],"tagged":true}));
                }
            }
        }
    }
    Ok(original)
}

pub(crate) fn empty_workload() -> Value {
    json!({"schema_version":4,"generators":[],"controls":[]})
}
