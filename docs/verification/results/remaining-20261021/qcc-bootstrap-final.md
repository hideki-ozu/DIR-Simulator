# Qcc bootstrapの回帰修正

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-qcc-bootstrap-20261022`
文書状態：未commit・未公開。記載した限定範囲の独立レビュー記録。

Fixed the two original normal Qcc integration regressions, preserving bilateral bootstrap, epoch 2 and every original assertion. All 20 normal tests PASS. After explicit parent scope expansion in questions.md, applied the same ordinary-LLDP fixture correction to the shipped raw sample; both ignored raw gates now PASS, including fresh-process restore and all five rehashed source/certificate mutant rejections. Historical failures remain preserved.

# Architecture

The correction changes only Net.a.tx's default_priority from 0 to 7. Qcc initially closes this managed output's class 0, so ordinary untagged LLDP needs class 7 to bootstrap. Net.b is unmanaged and can send ordinary class 0 LLDP. B retains default 0 to preserve the incoming untagged slow stream's class 0 contract.

Existing NetworkState::source_offer constructs source classification from frame.row.priority (runtime/network.rs:1365), so the slow source stays class 0 and untagged. Qcc's classification proof requires the untagged listener default to match generator priority (input/network/configuration.rs:333-343). LLDP uses merge_lldp_priority and ordinary retained TAS permissions (runtime/network/mac_merge/integration.rs:339-395). These existing guards stay intact; no production exemption or new abstraction was introduced.

The helper and raw sample need separate corrections: the two ignored raw tests directly prepare examples/network-composite/qcc-raw/model.ini, rather than calling merge_qcc_values. Parent authorization in raw-fixture-authorization.md expanded the initial tests-only scope to that model.json and its existing README.md. The sample's workload, bilateral epoch 2 expectation, raw PHY parameters and all assertions remain unchanged.

# Files

Exactly three repository files edited by this worker:

- /tmp/dir-remaining-implementation-20261021/crates/dir-simulator/tests/network_qcc.rs:764 —10-line insertion within merge_qcc_values; sets only A's default 7.
- /tmp/dir-remaining-implementation-20261021/examples/network-composite/qcc-raw/model.json:141 —one value, A's default 0->7.
- /tmp/dir-remaining-implementation-20261021/examples/network-composite/qcc-raw/README.md:5 —one paragraph explaining the bootstrap and retained source/listener contract.

Evidence root: /tmp/dir-remaining-implementation-20261021-evidence/qcc-bootstrap/

- network_qcc.frozen.rs, qcc-raw-model.frozen.json, qcc-raw-README.frozen.md: final owned-source snapshots.
- network_qcc.diff, qcc-raw-model.diff, qcc-raw-README.diff: reviewed baseline-to-final diffs. These sources were already untracked in the parent's worktree; saved baselines are the diff references.
- baseline/: original source/log/sample and the supplied failing repro. The original normal log shows18 pass,2 fail,2 ignored.
- handoff-before-raw-authorization.md: preserved prior report, when raw sample edits were outside scope and both raw gates still failed.
- bin/network_qcc-final-release: copied executable used for all final normal/raw runs and current_exe child workers.
- build-and-source.sha256 and handoff.sha256: exact source, binary, diff, library and report hashes.
- release-run-source-hashes.json, raw-fixed-start-source-hashes.json and source-drift-check.json: exact source/fixture boundaries.
- successful-and-diagnostic-commands.txt, release-test-commands.txt, raw-fixed-test-commands.txt: exact commands and exclusive short TMPDIRs.
- release-test-results.json: normal PASS plus original raw failures; raw-fixed-test-results.json: both corrected raw gates PASS.
- independent-observations.json / verify_evidence.py: independent normal bound and actual output checks.
- raw-fixed-independent-observations.json / verify_raw_fixed.py: independent raw bound, actual bilateral packets, certificate and rehashed-mutant checks.
- raw-startup-observations.json and raw-checkpoint-observations.json: original raw failure evidence retained unchanged.

Owned test SHA-256: d8ca165d42870c237e053853b15f26ac1a752d81a6bda33cc1b7e8fa47d3255c

Copied release executable SHA-256: 9e5069059b779daa7a064f213d1bce33d0df9f882112d87eac0d5f697a22f4c5

Release library SHA-256: ec85afbd55f77b729b2faf3d50a3c7b3efba06f859720671c373a5a1b051cab4

# Interfaces

Both original tests PASS unchanged:

- qcc_bounded_merge_lldp_verify_respond_admits_real_loaded_output
- qcc_merge_overloaded_burst_rejects_transaction_and_preserves_protocol_run

Both other helper consumers PASS: qcc_merge_pending_certificate_resumes_in_fresh_process and qcc_control_actual_starts_survive_multi_period_express_delay_and_response_carry_in. All other normal network_qcc tests, including existing source-admission/reclassification guards, PASS.

Both ignored raw tests PASS without modifying their assertions:

- qcc_raw_certified_startup_installs_and_delivers_actual_sampled_stream
- qcc_raw_fresh_process_certificates_and_rehashed_source_mutants

The second gate executes original and restored sessions plus five rejection workers for certificate, configuration hash, epoch, pmax and identity mutants. Independent evidence checks confirm every modified state.json has a correct updated manifest byte count/SHA-256 and each rejection worker passes, so the mutants are not merely rejected for an unrehashed file digest.

# Constraints

No subagents, Git mutation/publication, original-checkout changes, memory writes, production/input/output/PHY/preemption changes, broad formatting or workspace test run. Only the explicitly expanded sample scope was changed. Existing unrelated worktree changes were preserved. No applicable filesystem AGENTS.md was found; supplied session instructions were followed.

Rustc1.85.0 (4d91de4e4 2025-02-17), Cargo1.85.0. Direct rustc initially reused the normal debug rlib. One early release request waited for the parent's target lock and was canceled (exit 130) before building. The final release no-run completed in 3.66s, reusing the current release rlib and compiling this integration test. No library source file was newer than the release rlib at that build invocation. The executable was copied before all runs. JSON-only raw fixture correction required no new Cargo build: both raw tests load the corrected sample at runtime, as the parent requested. No running executable was overwritten.

Two discarded fixture candidates are retained: explicit PCP0 with untagged source membership failed prepare's tag-form check; setting both defaults7 with untagged slow data passed bootstrap but failed Qcc's classification proof. Those candidate debug runs were interrupted after decisive failures (exit 130). The final partial debug run passed the original success case, then was interrupted during the sampled test (exit 130) when the release executable became available. Only the complete release normal suite is claimed as 20/20 PASS.

The parent PHY worker subsequently edited runtime/ethernet/phy/medium.rs after the tested release library was built. Parent states that its later source17 change is PLCA-only. This report identifies the exact hashed executable boundary and does not claim to have verified the later integrated PHY source. The parent's final combined normal run remains the integration check for that later source cut. source-drift-check.json distinguishes the authorized raw model/README changes from shared production-source drift; all three owned files match their frozen copies.

# Acceptance criteria

- Both original normal failures: PASS.
- expected_topology_epoch 2 and every original assertion: unchanged. Removing the single inserted helper block reproduces baseline test source exactly.
- Both normal helper checkpoint/control consumers: PASS.
- All normal tests:20 pass,0 fail,2 ignored;16.01s; exit 0.
- Corrected shipped raw sample: both ignored raw gates PASS using the same copied release executable.
- Original normal/raw failures, final owned sources, diffs, exact commands, hashes and this report: retained.

# Verification

Exact final build/copy/check commands appear in successful-and-diagnostic-commands.txt. Final normal and initial raw commands appear in release-test-commands.txt; corrected raw commands appear in raw-fixed-test-commands.txt. Each invocation uses a distinct /tmp/dq-XXXX and retained DIR_QCC_EVIDENCE root. The owned test passes rustfmt --edition2024 --check, and all three owned paths pass the focused diff check. Source review confirms only the10-line helper insertion, one raw JSON value and one documentation paragraph changed.

Normal observations, independently checked across successful, checkpoint-fixture and overload scenarios:

- Both directions: LLDP SOF0/arrival576,000ps; Verify SOF672,000/arrival1,248,000ps; Respond SOF1,344,000/arrival1,920,000ps; Verified before50,000,000ps prepare.
- A LLDP ordinary class 7, B LLDP ordinary class 0. Actual installed plan topology_epoch 2 at10,000,000,000ps.
- Slow source priority 0, null tag,1018B MAC; SOF20,000,000,000ps; one certified delivery.
- Overload rejects infeasible_frame_capacity at50,000,000ps, never installs, and protocol execution continues through time_limit.

Independent normal arithmetic imports no simulator code:

- Maximum reserved MAC14+4+1000+4=1022B; ceil(1022/60)=18 fragment allowances.
- D=(1022+20)*8000+(18*24+20)*8000=11,952,000ps.
- B=(2048+32+12)*8000=16,736,000ps.
- Each control occupies(64+8+12)*8000=672,000ps at1Gb/s.
- L=4,V=15,R=17; x=D+B+(4+15+17)*672,000=52,880,000ps.

Actual installed normal slot:[0,52,880,000)ps. Original limits4/15/17 remain. Actual A control starts inside the managed slot are0 for each kind because ordinary class 7 LLDP waits for its gate. Data uses one pMAC Start packet; this run does not demonstrate a continuation fragment. Its unchanged fragmentation allowance remains conservative.

Independent raw arithmetic and observations:

- Declared200B payload reserves222B MAC; four minimum-fragment allowances give D=2,864,000ps. B=16,736,000ps.
- Each control occupies672,000ps plus8,000ps admission alignment =680,000ps.
- Cadence displacement at most15 samples on750MBd/16 grid gives1,250ps; plus4,080,000ps renderer bound and1ps rounding gives jitter4,081,251ps.
- L=4,V=15,R=17; occupied slot=44,080,000ps. Actual slot:[1,000,000,45,080,000)ps.
- With1,000,000ps assigned source offset, latency bound=1,000,000+44,080,000+4,081,251=49,161,251ps, matching the installed plan independently.
- Installed topology_epoch 2; physical bilateral_ready and initial_idle_verified true; both Merge certificate directions enabled/verified. Real actual_xmii LLDP/Verify/Respond decoded on both directions before prepare.
- Raw slow remains priority 0, untagged184B MAC. Source SOF6,101,000,000ps; decoded delivery6,105,000,000ps; both within_certificate true.
- Fresh-process pending raw certificate exists at topology_epoch 2; all five correctly rehashed mutants rejected.

Historical raw failures preserved:

1. Initial startup FAIL, 25.16s, exit 101: topology_epoch1 at4,773,000,000ps; stale_topology rejection at6,020,000,000ps; inactive_stream at6,100,000,000ps. logs/release-raw-startup.log and release-raw-startup-runs/dir-qcc-prepare-47-0/raw/results.json.
2. Initial checkpoint FAIL, 57.90s, exit 101: saved topology_epoch1, pending:null, version0; assertion before the five mutant checks. logs/release-raw-checkpoint.log and release-raw-checkpoint-runs/dir-qcc-prepare-11-0/positive/bundle/state.json.

The corrected raw startup and checkpoint elapsed times/exit codes are in raw-fixed-test-results.json. Parent questions.md was read and its explicit raw-fixture scope expansion applied; the authorization is copied in raw-fixture-authorization.md.
