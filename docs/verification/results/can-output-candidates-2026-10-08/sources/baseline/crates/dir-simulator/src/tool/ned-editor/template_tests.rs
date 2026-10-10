use super::*;
use crate::input::prepare_with_source;
use crate::types::Schedule;

fn cwd() -> std::path::PathBuf {
    std::env::temp_dir().join(random_id("ned-editor-template-test-").unwrap())
}

#[test]
fn starters_prepare_with_all_settings_and_zero_traffic_without_disk_creation() {
    let cwd = cwd();
    assert!(!cwd.exists());
    for (id, expected_profile, count) in [
        ("multibus", "can.cc.multibus.v1", 4),
        ("can", "can.cc.ideal.v1", 3),
    ] {
        let project = new_project(id, "Starter", &cwd).unwrap();
        let prepared =
            prepare_with_source(&project.config, &project.cwd, &project.source()).unwrap();
        assert_eq!(prepared.common.profile, expected_profile);
        assert_eq!(prepared.common.network, "Starter.Main");
        assert_eq!(prepared.can.controllers.len(), 2);
        assert_eq!(prepared.can.buses.len(), 1);
        assert_eq!(prepared.common.channel_count, 4);
        assert!(prepared.gateway.gateways.is_empty());
        assert_eq!(project.files.len(), count);
        assert_eq!(prepared.can.generators.len(), 2);
        assert_eq!(
            prepared.can.generators[0].source,
            prepared.can.generators[1].source
        );
        assert_ne!(
            prepared.can.generators[0].frame.id,
            prepared.can.generators[1].frame.id
        );
        for generator in &prepared.can.generators {
            assert_eq!(generator.schedule.time(0), None);
        }
        assert!(
            matches!(&prepared.can.generators[0].schedule, Schedule::Explicit(times) if times.is_empty())
        );
        assert!(matches!(
            &prepared.can.generators[1].schedule,
            Schedule::Periodic {
                count: Some(0),
                end: Some(_),
                ..
            }
        ));
        for key in [
            "network",
            "ned-path",
            "sim-time-limit",
            "metrics-window",
            "max-events",
            "max-delta-cycles",
            "model-profile",
            "workload",
        ] {
            assert!(project.header.general.contains_key(key), "missing {key}");
        }
        assert_eq!(
            project.header.general.contains_key("model-config"),
            id == "multibus"
        );
        for node in ["a", "b"] {
            for parameter in [
                "queueCapacity",
                "txProcessingDelay",
                "rxProcessingDelay",
                "rxFilter",
            ] {
                assert!(
                    project
                        .header
                        .general
                        .contains_key(&format!("Main.{node}.{parameter}"))
                );
            }
        }
        assert!(project.header.general.contains_key("Main.bus.bitrate"));
        assert!(project.header.general.contains_key("Main.bus.profile"));
        assert_eq!(project.header.channels["Main::a.tx"]["delay"], "1us");
        assert_eq!(
            prepared
                .can
                .controllers
                .iter()
                .find(|c| c.id == "Main.a")
                .unwrap()
                .tx_channel_ps,
            1_000_000
        );
        let declarations = project.parsed.values().next().unwrap().declarations();
        assert_eq!(declarations.len(), 8);
        for name in STANDARD_NAMES {
            assert!(
                declarations
                    .iter()
                    .any(|d| d.name() == format!("Starter.{name}"))
            );
        }
        assert!(!project.config.parent().unwrap().exists());
    }
    assert!(!cwd.exists());
}

#[test]
fn gateway_reference_is_exposed_and_prepares_complete_routing() {
    assert_eq!(templates()[0]["id"], "multibus");
    let ids: BTreeSet<_> = templates()
        .into_iter()
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from([
            "can".into(),
            "multibus".into(),
            "multibus-gateway".into(),
            "multibus-empty".into()
        ])
    );
    let project = new_project("multibus-gateway", "GatewayReference", &cwd()).unwrap();
    let prepared = prepare_with_source(&project.config, &project.cwd, &project.source()).unwrap();
    assert_eq!(prepared.can.controllers.len(), 4);
    assert_eq!(prepared.can.buses.len(), 2);
    assert_eq!(prepared.common.channel_count, 8);
    assert_eq!(prepared.gateway.gateways.len(), 1);
    let gateway = &prepared.gateway.gateways[0];
    assert_eq!(gateway.node, "Main.gw");
    assert_eq!(gateway.ports.len(), 2);
    assert_eq!(gateway.processing_delay_ps, 10_000_000);
    assert_eq!(gateway.rx_queue_capacity, 64);
    assert_eq!(gateway.hop_limit, 16);
    assert_eq!(gateway.routes.len(), 1);
    let route = &gateway.routes[0];
    assert_eq!(route.id, "forward");
    assert_eq!(route.format, "standard");
    assert_eq!(route.id_min, 256);
    assert_eq!(route.id_max, 271);
    assert_eq!(route.egress.len(), 1);
    assert!(
        prepared
            .can
            .generators
            .iter()
            .all(|g| g.schedule.time(0).is_none())
    );
}

#[test]
fn snapshot_provenance_paths_hashes_roles_and_layouts_are_consistent() {
    let cwd = cwd();
    let project = new_project("multibus", "Fresh", &cwd).unwrap();
    let another = new_project("multibus", "Fresh", &cwd).unwrap();
    assert_ne!(project.id, another.id);
    assert_ne!(project.config, another.config);
    assert!(
        matches!(&project.origin, ProjectOrigin::New { template, name } if template == "multibus" && name == "Fresh")
    );
    assert!(project.config.starts_with(cwd.join(".ned-editor-drafts")));
    assert_eq!(project.header.config, project.config);
    assert_eq!(project.header.cwd, project.cwd);
    for file in project.files.values() {
        assert!(file.path.is_absolute());
        assert!(file.path.starts_with(project.config.parent().unwrap()));
        assert_eq!(
            file.id,
            format!("f{}", &hash(file.path.to_string_lossy().as_bytes())[..24])
        );
        assert_eq!(file.hash, hash(file.text.as_bytes()));
        assert_eq!(file.hash, file.origin_hash);
        assert_eq!(file.text, file.origin_text);
        assert!(file.stat.is_none());
    }
    let ned = project.ned_files().next().unwrap();
    assert_eq!(
        ned.roles,
        vec![FileRole::Ned {
            root_index: 0,
            relative: "Fresh/Main.ned".into(),
            package: "Fresh".into()
        }]
    );
    assert_eq!(ned.path, project.header.roots[0].join("Fresh/Main.ned"));
    let layout = &project.layouts[&ned.id];
    assert_eq!(layout.path, ned.path.with_file_name("Main.ned.layout.json"));
    assert!(layout.raw.is_none() && layout.hash.is_none() && layout.stat.is_none());
    assert!(layout.adopted.is_none() && layout.warning.is_none());
    assert_eq!(project.parsed.len(), 1);
    assert!(!cwd.exists());
}

#[test]
fn invalid_names_templates_and_relative_cwd_are_rejected() {
    let cwd = cwd();
    for name in [
        "",
        "a.b",
        "../Escape",
        "a-b",
        "a b",
        "1project",
        "プロジェクト",
        "network",
        "package",
        "default",
        "true",
        "moduleinterface",
    ] {
        assert_eq!(
            new_project("multibus", name, &cwd).unwrap_err().code,
            "E-EDITOR-TEMPLATE"
        );
    }
    assert!(new_project("missing", "Valid", &cwd).is_err());
    assert!(new_project("can", "Valid", Path::new("relative")).is_err());
    for name in [
        "_Project1",
        "__PACKAGE__",
        "__PROFILE__",
        "__MODEL_CONFIG__",
    ] {
        let project = new_project("multibus", name, &cwd).unwrap();
        prepare_with_source(&project.config, &project.cwd, &project.source()).unwrap();
    }
    assert!(!cwd.exists());
}

#[test]
fn catalog_materialization_allocates_fresh_parseable_definitions_for_both_profiles() {
    let entries = catalog();
    assert_eq!(entries.len(), 7);
    assert_eq!(
        entries
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        7
    );
    for multibus in [false, true] {
        let project =
            new_project(if multibus { "multibus" } else { "can" }, "Starter", &cwd()).unwrap();
        let ned = project.ned_files().next().unwrap();
        let existing: BTreeSet<_> = STANDARD_NAMES
            .into_iter()
            .map(|n| format!("Starter.{n}"))
            .chain(["Starter.Controller_2".into(), "MultibusController_2".into()])
            .collect();
        for entry in &entries {
            for key in ["id", "name", "label", "description", "kind"] {
                assert!(entry[key].as_str().is_some_and(|s| !s.is_empty()));
            }
            let id = entry["id"].as_str().unwrap();
            let (root, declarations) = materialize(id, "Starter", multibus, &existing).unwrap();
            assert!(!existing.contains(&root));
            assert!(!declarations.contains("package "));
            let raw = format!("package Starter;\n{declarations}");
            let parsed = parse_ned(&raw, Path::new("Catalog.ned"), "Starter").unwrap();
            assert!(parsed.declarations().iter().any(|d| d.name() == root));
            for d in parsed.declarations() {
                assert!(!existing.contains(d.name()));
                assert!(!existing.contains(d.name().rsplit('.').next().unwrap()));
                for (_, child_type) in d.children() {
                    assert!(
                        parsed
                            .declarations()
                            .iter()
                            .any(|c| c.name() == *child_type)
                    );
                }
            }
            let root_declaration = parsed
                .declarations()
                .iter()
                .find(|d| d.name() == root)
                .unwrap();
            assert_eq!(root_declaration.kind(), entry["kind"].as_str().unwrap());
            if entry["name"] == "Controller" || entry["name"] == "Bus" {
                assert_eq!(
                    root_declaration
                        .implementation()
                        .unwrap()
                        .contains("Multibus"),
                    multibus
                );
            }
            let mut source = project.source();
            source.files.insert(
                ned.path.clone(),
                format!("{}\n{declarations}", ned.text).into(),
            );
            prepare_with_source(&project.config, &project.cwd, &source).unwrap();
        }
    }
}

#[test]
fn modules_use_fresh_multibus_dependencies_and_stable_catalog_ids() {
    for (id, ports) in [("@builtin:Gateway", 2), ("@builtin:Fanout", 3)] {
        let existing = BTreeSet::from([
            "demo.Gateway".into(),
            "demo.Fanout".into(),
            "demo.MultibusController".into(),
            "MultibusController_2".into(),
        ]);
        let (root, text) = materialize(id, "demo", false, &existing).unwrap();
        let parsed = parse_ned(
            &format!("package demo;\n{text}"),
            Path::new("Types.ned"),
            "demo",
        )
        .unwrap();
        assert_eq!(parsed.declarations().len(), 2);
        let dependency = &parsed.declarations()[0];
        assert_eq!(dependency.name(), "demo.MultibusController_3");
        assert_eq!(
            dependency.implementation(),
            Some("dir.can.MultibusController")
        );
        let module = &parsed.declarations()[1];
        assert_eq!(module.name(), root);
        assert_eq!(module.children().len(), ports);
        assert!(
            module
                .children()
                .iter()
                .all(|(_, name)| name == dependency.name())
        );
        assert!(requires_multibus(id));
    }
    for id in ["@builtin:MultibusController", "@builtin:MultibusBus"] {
        assert!(requires_multibus(id));
        let (_, text) = materialize(id, "demo", false, &BTreeSet::new()).unwrap();
        assert!(text.contains("dir.can.Multibus"));
    }
    for id in [
        "@builtin:Controller",
        "@builtin:Bus",
        "@builtin:FixedDelay",
        "Gateway",
        "demo.Gateway",
    ] {
        assert!(!requires_multibus(id));
    }
    for id in ["Controller", "demo.Controller", "@builtin:Unknown"] {
        assert!(materialize(id, "demo", false, &BTreeSet::new()).is_err());
    }
    assert!(materialize("@builtin:Bus", "invalid.package", false, &BTreeSet::new()).is_err());
    assert!(materialize("@builtin:Gateway", "", false, &BTreeSet::new()).is_err());
}
