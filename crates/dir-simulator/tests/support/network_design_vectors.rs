// Exact fixture input checks; expected values are used only by assertions.
mod network_design_vectors {
    use super::*;
    fn vector(id: &str) -> Value {
        let f: Value = serde_json::from_slice(
            &fs::read(root().join("docs/proposals/fixtures/network-extension-v1.json")).unwrap(),
        )
        .unwrap();
        f["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .unwrap()
            .clone()
    }
    fn receipt(v: &Value, observed: &Value, test: &str, basis: &str) {
        assert_eq!(observed, &v["expected"], "{}", v["id"]);
        let p = root().join("tmp/extension-implementation-20261017/network-vector-receipts");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join(format!("{}.json", v["id"].as_str().unwrap())), serde_json::to_vec_pretty(&json!({"id":v["id"],"status":"passed","test":test,"basis":basis,"observed":observed,"asserted_fields":v["expected"].as_object().unwrap().keys().collect::<Vec<_>>()})).unwrap()).unwrap();
    }
    #[test]
    fn exact_prepare_rejection_vectors() {
        for (id, name) in [
            ("P-COMPOSITE-T01-V01", "composite"),
            ("P-REPLICATION-T01-V01", "redundancy"),
            ("P-PREEMPTION-T01-V01", "preemption"),
        ] {
            let v = vector(id);
            let mut rejected = 0;
            let mut callbacks = 0;
            let mut case_evidence = vec![];
            for case in v["input"]["cases"].as_array().unwrap() {
                let (mut c, mut w) = sample(name);
                if name == "composite" {
                    let pointer = case["pointer"].as_str().unwrap();
                    let value = case.get("value").cloned().unwrap_or_else(|| json!({}));
                    if let Some(slot) = c.pointer_mut(pointer) {
                        *slot = value;
                    } else {
                        let (parent, key) = pointer.rsplit_once('/').unwrap();
                        c.pointer_mut(parent)
                            .unwrap()
                            .as_object_mut()
                            .unwrap()
                            .insert(key.into(), value);
                    }
                } else if name == "redundancy" {
                    let stream = &mut c["replication"]["streams"][0];
                    match case["field"].as_str().unwrap() {
                        "history_capacity" => stream["history_capacity"] = case["value"].clone(),
                        "egress_paths.length" => {
                            let n = case["value"].as_u64().unwrap() as usize;
                            let original = stream["egress_paths"][0].clone();
                            stream["egress_paths"] = json!(
                                (0..n)
                                    .map(|i| {
                                        let mut p = original.clone();
                                        p["id"] = json!(format!("path{i}"));
                                        p
                                    })
                                    .collect::<Vec<_>>()
                            );
                        }
                        "ports" => {
                            // Instantiate fixture port symbols on the existing topology.
                            stream["egress_paths"][0]["ports"] = json!(
                                case["value"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|port| match port.as_str().unwrap() {
                                        "A.tx" => "Net.a.tx",
                                        "B.tx" => "Net.s1.tx_a",
                                        p => panic!("unknown fixture symbol {p}"),
                                    })
                                    .collect::<Vec<_>>()
                            );
                        }
                        "egress_paths" => {
                            stream["egress_paths"][1]["ports"] =
                                stream["egress_paths"][0]["ports"].clone()
                        }
                        "controls" => w["controls"] = case["value"].clone(),
                        x => panic!("unhandled {x}"),
                    }
                } else {
                    match case["field"].as_str().unwrap() {
                        "fragment_payload_bytes" | "express_classes" => {
                            c["preemption"]["outputs"][0][case["field"].as_str().unwrap()] =
                                case["value"].clone()
                        }
                        "class_partition" if case["violation"] == "overlap" => {
                            c["preemption"]["outputs"][0]["preemptable_classes"] =
                                json!([0, 1, 2, 3, 4, 5, 6, 7])
                        }
                        "class_partition" => {
                            c["preemption"]["outputs"][0]["preemptable_classes"] =
                                json!([0, 1, 2, 3, 4, 5])
                        }
                        "cbs.priority" => {
                            c["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}])
                        }
                        "bitrate_bps" => {} // Applied to the real NED channel below.
                        "duplex" => {
                            // This static profile has no half-duplex output declaration;
                            // its strict schema rejects the explicit unsupported setting.
                            c["preemption"]["outputs"][0]["duplex"] = case["value"].clone();
                        }
                        "unexpected" => {
                            c["preemption"]["outputs"][0]["unexpected"] = case["value"].clone()
                        }
                        x => panic!("unhandled {x}"),
                    }
                }
                let f = fixture(name, &c, &w);
                if name == "preemption" && case["field"] == "bitrate_bps" {
                    let model = fs::read_to_string(
                        root().join("examples/ethernet/tsn/models/ethtsn/Main.ned"),
                    )
                    .unwrap();
                    let models = f.0.join("models/ethtsn");
                    fs::create_dir_all(&models).unwrap();
                    fs::write(
                        models.join("Main.ned"),
                        model.replace(
                            "default(1Gbps)",
                            &format!("default({}bps)", case["value"].as_str().unwrap()),
                        ),
                    )
                    .unwrap();
                    let ini = fs::read_to_string(f.0.join("run.ini")).unwrap().replace(
                        root()
                            .join("examples/ethernet/tsn/models")
                            .to_str()
                            .unwrap(),
                        f.0.join("models").to_str().unwrap(),
                    );
                    fs::write(f.0.join("run.ini"), ini).unwrap();
                }
                let result = prepare(&f.0.join("run.ini"));
                assert!(result.is_err(), "{id}: {case}");
                let error = result.as_ref().unwrap_err();
                case_evidence.push(json!({"input":case,"reason":error.reason,"message":error.message,"target":error.target,"source":error.source,"line":error.line}));
                rejected += usize::from(result.is_err());
                // Use the real failure publication path for its committed-callback
                // count, rather than inferring a zero from the prepare error.
                let report =
                    dir_simulator::run_config(&f.0.join("run.ini"), &f.0.join("rejected-output"))
                        .unwrap();
                assert_eq!(report.termination, "prep_failed");
                callbacks += report.committed_events.parse::<usize>().unwrap();
            }
            receipt(
                &v,
                &json!({"all_prepare_rejected":rejected == v["input"]["cases"].as_array().unwrap().len(),"callbacks_committed":callbacks}),
                "network_design_vectors::exact_prepare_rejection_vectors",
                "prepare(config/workload) plus actual run_config failure-report committed_events",
            );
            let path = root()
                .join("tmp/extension-implementation-20261017/network-vector-receipts")
                .join(format!("{id}.json"));
            let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            saved["case_evidence"] = json!(case_evidence);
            saved["input_mapping"] = json!({"A.tx":"Net.a.tx","B.tx":"Net.s1.tx_a","bitrate_bps":"NED Wire.bitrate","duplex":"unsupported explicit half-duplex output setting"});
            fs::write(path, serde_json::to_vec_pretty(&saved).unwrap()).unwrap();
        }
    }
    #[test]
    fn exact_legal_preemption_timeline_vector() {
        let v = vector("P-PREEMPTION-T02-V02");
        let i = &v["input"];
        let (mut c, mut w) = sample("preemption");
        for o in c["preemption"]["outputs"].as_array_mut().unwrap() {
            o["fragment_payload_bytes"] = i["fragment_payload_bytes"].clone();
            o["fragment_overhead_bytes"] = i["fragment_overhead_bytes"].clone();
            o["resume_gap_ps"] = i["resume_gap_ps"].clone();
        }
        w["generators"][1]["times_ps"] = json!([i["express_arrival_ps"]]);
        let r = product("preemption", &c, &w, 2_000_000);
        let mut fragments = rows(&r, "dir.preemption.fragment");
        fragments.sort_by_key(|r| r["ordinal"].as_str().unwrap().parse::<usize>().unwrap());
        let express = rows(&r, "dir.preemption.transfer")
            .into_iter()
            .find(|r| r["kind"] == "express")
            .unwrap();
        let gaps = rows(&r, "dir.preemption.gap");
        let observed = json!({"fragment_intervals_ps":fragments.iter().map(|r|json!([r["start_ps"],r["end_ps"]])).collect::<Vec<_>>(),"express_interval_ps":[express["sof_ps"],express["release_ps"]],"express_eof_ps":express["eof_ps"],"gap_interval_ps":[gaps[0]["start_ps"],gaps[0]["end_ps"]],"frame_eof_ps":slow(&r)["eof_ps"],"fragment_busy_ps":slow(&r)["occupied_ps"],"express_busy_ps":express["occupied_ps"],"total_busy_ps":metric(&r,"Net.a.tx","preemption.port.busy_ps"),"total_gap_ps":metric(&r,"Net.a.tx","preemption.port.resume_gap_ps")});
        receipt(
            &v,
            &observed,
            "network_design_vectors::exact_legal_preemption_timeline_vector",
            "run + exported actual fragment/transfer/gap rows and physical port summary",
        );
    }
}
