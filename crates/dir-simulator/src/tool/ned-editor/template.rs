//! Compiled standard declarations and prospective projects; no filesystem I/O.
use super::input::{
    CapturedFile, FileRole, LayoutCapture, ProjectOrigin, ProjectSnapshot, SnapshotInputSource,
};
use super::{EditorError, Result, absolute, hash, random_id};
use crate::input::{identifier, inspect_config, parse_ned, reserved};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

const CONTROLLER: &str = include_str!("templates/controller.ned");
const BUS: &str = include_str!("templates/bus.ned");
const GATEWAY: &str = include_str!("templates/gateway.ned");
const FANOUT: &str = include_str!("templates/fanout.ned");
const FIXED_DELAY: &str = include_str!("templates/fixed-delay.ned");
const STANDARD_NAMES: [&str; 7] = [
    "Controller",
    "Bus",
    "MultibusController",
    "MultibusBus",
    "Gateway",
    "Fanout",
    "FixedDelay",
];

pub(crate) fn catalog() -> Vec<Value> {
    [
        (
            "Controller",
            "simple",
            "Controller",
            "現在のプロファイルに合わせたCAN Controller。",
        ),
        (
            "Bus",
            "simple",
            "Bus",
            "現在のプロファイルに合わせた2ポートのCAN Bus。",
        ),
        (
            "MultibusController",
            "simple",
            "Multibus Controller",
            "Multibus用のController。単一CANではプロファイル変更を確認します。",
        ),
        (
            "MultibusBus",
            "simple",
            "Multibus Bus",
            "Multibus用の2ポートBus。単一CANではプロファイル変更を確認します。",
        ),
        (
            "Gateway",
            "module",
            "Gateway",
            "2ポートのGateway。経路はGateway設定フォームで編集します。",
        ),
        (
            "Fanout",
            "module",
            "Fanout",
            "3ポートのFanout。Gateway設定で複数の出口を選択します。",
        ),
        (
            "FixedDelay",
            "channel",
            "Fixed Delay",
            "伝搬遅延を設定できるFixedDelay channel。",
        ),
    ]
    .into_iter()
    .map(|(name, kind, label, description)| {
        json!({
            "id": format!("@builtin:{name}"), "kind": kind,
            "name": name, "label": label, "description": description,
        })
    })
    .collect()
}

/// The first entry is the default New project. The reference is independently runnable.
pub(crate) fn templates() -> Vec<Value> {
    vec![
        json!({"id":"multibus", "name":"Multibus", "description":"Controller 2個とBus 1個。全設定項目を用意し、初期送信は無効。"}),
        json!({"id":"can", "name":"CAN", "description":"単一CAN。Controller 2個とBus 1個、初期送信は無効。"}),
        json!({"id":"multibus-gateway", "name":"Multibus Gateway設定例", "description":"Bus 2個とGateway。経路の全設定項目を用意し、初期送信は無効。"}),
        json!({"id":"multibus-empty", "name":"空のMultibusネットワーク", "description":"moduleとBusを配置して組み立てます。接続が完成してから検証・保存できます。"}),
    ]
}

pub(crate) fn requires_multibus(id: &str) -> bool {
    matches!(
        id,
        "@builtin:Gateway"
            | "@builtin:Fanout"
            | "@builtin:MultibusController"
            | "@builtin:MultibusBus"
    )
}

fn invalid(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-TEMPLATE", message, 422)
}

fn valid_identifier(name: &str) -> bool {
    identifier(name) && !reserved(name)
}

fn qualified(package: &str, name: &str) -> String {
    format!("{package}.{name}")
}

fn profile(multibus: bool) -> &'static str {
    if multibus {
        "can.cc.multibus.v1"
    } else {
        "can.cc.ideal.v1"
    }
}

fn controller(name: &str, multibus: bool) -> String {
    CONTROLLER.replace("__NAME__", name).replace(
        "__CLASS__",
        if multibus {
            "dir.can.MultibusController"
        } else {
            "dir.can.Controller"
        },
    )
}

fn bus(name: &str, multibus: bool) -> String {
    BUS.replace("__NAME__", name)
        .replace(
            "__CLASS__",
            if multibus {
                "dir.can.MultibusBus"
            } else {
                "dir.can.Bus"
            },
        )
        .replace("__PROFILE__", profile(multibus))
}

fn compound(template: &str, name: &str, controller: &str) -> String {
    template
        .replace("__NAME__", name)
        .replace("__CONTROLLER__", controller)
}

/// Always allocate fresh declarations, including module dependencies. The caller
/// may provide either local declaration names or fully qualified type names.
pub(crate) fn materialize(
    id: &str,
    package: &str,
    multibus: bool,
    existing_names: &BTreeSet<String>,
) -> Result<(String, String)> {
    let standard = id
        .strip_prefix("@builtin:")
        .filter(|name| STANDARD_NAMES.contains(name))
        .ok_or_else(|| invalid(format!("Unknown catalog ID: {id}")))?;
    if !package.split('.').all(valid_identifier) {
        return Err(invalid("Invalid NED package"));
    }
    let mut allocated = existing_names.clone();
    let mut allocate = |base: &str| {
        let mut name = base.to_owned();
        let mut suffix = 2u64;
        while allocated.contains(&name) || allocated.contains(&qualified(package, &name)) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        allocated.insert(name.clone());
        allocated.insert(qualified(package, &name));
        name
    };
    let name = allocate(standard);
    let declarations = match standard {
        "Controller" => controller(&name, multibus),
        "Bus" => bus(&name, multibus),
        "MultibusController" => controller(&name, true),
        "MultibusBus" => bus(&name, true),
        "FixedDelay" => FIXED_DELAY.replace("__NAME__", &name),
        "Gateway" | "Fanout" => {
            let dependency = allocate("MultibusController");
            let module = compound(
                if standard == "Gateway" {
                    GATEWAY
                } else {
                    FANOUT
                },
                &name,
                &qualified(package, &dependency),
            );
            format!("{}\n{module}", controller(&dependency, true))
        }
        _ => unreachable!("catalog ID checked above"),
    };
    Ok((qualified(package, &name), declarations))
}

fn standard_declarations(package: &str, multibus: bool) -> String {
    [
        controller("Controller", multibus),
        bus("Bus", multibus),
        controller("MultibusController", true),
        bus("MultibusBus", true),
        compound(
            GATEWAY,
            "Gateway",
            &qualified(package, "MultibusController"),
        ),
        compound(FANOUT, "Fanout", &qualified(package, "MultibusController")),
        FIXED_DELAY.replace("__NAME__", "FixedDelay"),
    ]
    .join("\n")
}

pub(crate) fn new_project(template: &str, name: &str, cwd: &Path) -> Result<ProjectSnapshot> {
    let (multibus, gateway) = match template {
        "multibus" => (true, false),
        "can" => (false, false),
        "multibus-gateway" => (true, true),
        "multibus-empty" => (true, false),
        _ => return Err(invalid(format!("Unknown project template: {template}"))),
    };
    if !valid_identifier(name) {
        return Err(invalid(
            "Project name must be an ASCII identifier and not a reserved NED token",
        ));
    }
    if !cwd.is_absolute() {
        return Err(invalid("New project cwd must be absolute"));
    }
    let cwd = absolute(cwd, cwd);
    let directory = cwd.join(".ned-editor-drafts").join(random_id("project-")?);
    let config = directory.join("project.ini");
    let mut ini = if gateway {
        include_str!("templates/gateway-starter.ini").replace("__PACKAGE__", name)
    } else {
        include_str!("templates/starter.ini")
            .replace("__PROFILE__", profile(multibus))
            .replace(
                "__MODEL_CONFIG__",
                if multibus {
                    "model-config = \"routing.json\"\n"
                } else {
                    ""
                },
            )
            .replace("__PACKAGE__", name)
    };
    if template == "multibus-empty" {
        ini = ini
            .lines()
            .take_while(|line| !line.starts_with("[Channel "))
            .filter(|line| !line.starts_with("Main."))
            .fold(String::new(), |mut text, line| {
                text.push_str(line);
                text.push('\n');
                text
            });
    }
    let header = inspect_config(&ini, &config, &cwd)
        .map_err(|e| EditorError::common("E-EDITOR-TEMPLATE", e))?;
    let network = if gateway {
        include_str!("templates/gateway-starter.ned")
    } else {
        include_str!("templates/starter.ned")
    };
    let ned = format!(
        "package {name};\n\n{}\n{}",
        standard_declarations(name, multibus),
        if template == "multibus-empty" {
            "network Main {}\n".into()
        } else {
            network.replace("__PACKAGE__", name)
        },
    );
    let relative = Path::new(name).join("Main.ned");
    let ned_path = header.roots[0].join(&relative);
    let parsed = parse_ned(&ned, &ned_path, name)
        .map_err(|e| EditorError::common("E-EDITOR-TEMPLATE", e))?;
    let mut files = BTreeMap::new();
    let mut capture = |path, role, text: String| {
        let text: Arc<str> = text.into();
        let digest = hash(text.as_bytes());
        let id = format!(
            "f{}",
            &hash(Path::new(&path).to_string_lossy().as_bytes())[..24]
        );
        files.insert(
            id.clone(),
            CapturedFile {
                id: id.clone(),
                path,
                roles: vec![role],
                text: text.clone(),
                hash: digest.clone(),
                origin_text: text,
                origin_hash: digest,
                stat: None,
            },
        );
        id
    };
    capture(config.clone(), FileRole::Config, ini);
    let ned_id = capture(
        ned_path.clone(),
        FileRole::Ned {
            root_index: 0,
            relative,
            package: name.into(),
        },
        ned,
    );
    capture(
        header.workload.clone().expect("embedded workload"),
        FileRole::Workload,
        if template == "multibus-empty" {
            "{\n  \"schema_version\": 1,\n  \"generators\": []\n}\n".into()
        } else {
            include_str!("templates/workload.json").into()
        },
    );
    if let Some(path) = &header.model_config {
        capture(
            path.clone(),
            FileRole::ModelConfig,
            if gateway {
                include_str!("templates/routing-gateway.json")
            } else {
                include_str!("templates/routing-empty.json")
            }
            .into(),
        );
    }
    let source = SnapshotInputSource::virtual_project(
        files
            .values()
            .map(|f| (f.path.clone(), f.text.clone()))
            .collect(),
        &header.roots,
    );
    let layout = LayoutCapture {
        path: ned_path.with_file_name("Main.ned.layout.json"),
        raw: None,
        hash: None,
        stat: None,
        adopted: None,
        warning: None,
    };
    Ok(ProjectSnapshot {
        origin: ProjectOrigin::New {
            template: template.into(),
            name: name.into(),
        },
        id: random_id("snapshot-")?,
        config,
        cwd,
        header,
        files,
        directories: source.directories,
        metadata: source.metadata,
        layouts: BTreeMap::from([(ned_id.clone(), layout)]),
        parsed: BTreeMap::from([(ned_id, parsed)]),
    })
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
