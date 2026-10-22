# Native保存と入力取得の最終レビュー

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-native-input-20261022`
文書状態：未commit・未公開。記載した限定範囲の独立レビュー記録。

Bounded correctness review completed: **no concrete remaining findings in the two assigned fixes**. The configured Merge Native P2 timing inconsistency and the specifically missing same-future wake positive are closed. The input reacquisition seam is coherent with its existing public APIs and recorded focused verification. This is not approval of parent final all-workspace gates.

# Architecture

The Merge production change is one expression: `crates/dir-simulator/src/runtime/network/mac_merge/journal.rs:748` now validates arrival using checked EOF + `physical_receive_delay(i)`. It agrees with the existing producer and private physical packet authority while retaining normalization, identity, duration, source and raw-path guards. No second serializer or timer was introduced.

The wake addition is a regression test of existing reset/identity renewal. The logical output wake is carried by the existing shared Control FES reservation. Its earlier 6us work and subsequent 10us reservation are correct; another per-output FES event is neither required nor warranted.

Input ordinary/reference preparation now shares the acquisition and dispatch at `input.rs:853`. It forwards the actual parsed `Ini` and captured snapshot vector into either the builtin continuation (`input.rs:885`) or registry continuation (`registry/prepare.rs:31`). Explicit registry entry acquires once at `registry/prepare.rs:25` and forwards the same values through builtin branches at lines 84 and 98. There is no reacquiring wrapper or cache masking an extra read.

# Files

Reviewed current narrow sections of:

- `crates/dir-simulator/src/runtime/network/mac_merge/journal.rs` and `preemption.rs`, against the supplied worker diffs.
- `crates/dir-simulator/src/input.rs`, `input/references.rs`, `registry/prepare.rs`, and `registry/mod.rs`, using read-only Git diff and the recorded worker-baseline diff to separate the acquisition fix from previously integrated features.

Read `merge-native-wake-fix/{handoff.md,journal-final.diff,preemption-final.diff,verification.json}`, actual focused logs, input process log and questions, preserved `input-acquisition-regression/final-report-draft.md`, the AcquireOnce harness, and the prior wire closure review. The overwritten CLI final summary was not treated as the detailed report.

No severity/path/line defect entries remain to report. No actionable early finding required a questions-file append.

# Interfaces

**Native timing:** The actual configured test (`journal.rs:1075`) uses source TX 30ns + propagation 17ns + peer RX 50ns, totaling 97ns. Recorded packet 6 has EOF 5.536us and arrival 5.633us. Authentic Native restoration, checkpoint equality and actual FES reservation validation pass. The mutant changes the retained packet and matching journal occurrences together by +1ps, then fails the precise physical identity/timing guard. The original backend checkpoint, live timers, sequence and FES keys remain unchanged and its reservations still validate. This is adequate for the reported private timing inconsistency; it does not claim a new public adoption-path test.

**Same-future wake:** The actual Engine test (`preemption.rs:1384`) queues before 1us, applies source down/up at 1us, and holds the class gate until 10us. It records old 10us generation 9 / epoch 8 and replacement 10us generation 11 / epoch 11. The old token is invalid and applying its actual planned delta to restored state leaves the checkpoint unchanged. The replacement is valid; Native restoration and FES validation pass at reset. At actual 6us the shared live Control timer reserves 10us with the refreshed valid generation 12, and the queued data has actual SOF 10us. All required observations are asserted, rather than inferred solely from termination.

**Input:** Public signatures remain unchanged. `inspect_config` delegates to the same parsed-INI header extraction; annotations use its existing spans. Ordinary/reference builtin diagnostics share the original validation body and annotation boundary. Unknown/unparseable profile selection falls through to the existing builtin validator rather than silently dispatching to a registry. Registered reference inspection remains explicitly unsupported for reference extraction while returning the shared prepared result. Common builtin metrics/provenance finalization now runs for both ordinary and reference entrypoints; registry-specific finalization remains in place. The registry export is crate-private. No concrete diagnostics, provenance, or API regression was found in this seam.

# Constraints

No repository edits, builds, test executions, Git mutations, agents, original-checkout access, memory access, or secret access. Only this evidence report was written. Ancestor/repository instruction-file searches found no additional AGENTS.md files; supplied instructions were followed. No mutable PHY or Qcc implementation was reviewed and no prior accepted global feature was reopened. No claim is made that this already-running root session was switched to a different model/profile.

# Acceptance criteria

- Configured Merge Native P2: closed by current source plus authentic positive and coherent timing-negative evidence.
- Same-future wake positive: closed by actual source-driven Engine/Native/FES observations and data SOF.
- Input single acquisition, captured-input dispatch, legacy API and diagnostic/provenance consistency: accepted within the bounded seam and recorded tests.
- Parent final current-source workspace tests, current CLI, Clippy, documentation/browser gates and concurrent PHY/Qcc integration: **pending parent confirmation**, not closed by this review.

# Verification

Review only; no gates were rerun. Directly inspected recorded successful results:

- Native positive/negative 1; same-future wake 1; Merge private units 21 (includes Native); CBS units 3; raw Native guard 1: 26 distinct tests.
- Input units 64, including all 6 reference tests; registry unit 1; metadata-hash and frozen-unused checks 1 each.
- Existing integrations: registry 38, registry diagnostics 8, SoC execution 27, transaction fabric 10, legacy SoC memory control 3, SoC 20: 106 total.
- Evidence-only AcquireOnce: seven real-filesystem scenarios across ordinary/reference/explicit registry. The harness rejects repeated `read_utf8`, compares ordinary/reference prepared values and provenance, and compares explicit-registry captured raw inputs. It does not separately assert explicit-registry provenance equality; the report does not overstate that assertion. Existing reference tests supply the operation-trace/frozen-source coverage.

Read-only SHA-256 comparisons matched all 10 recorded entries: the two Merge/wake files, three recorded timing dependencies, four input/registry files, and unchanged reference tests. Dependency hashes were checked without reopening their accepted implementation or mutable PHY design. Hash agreement ties these reviewed files to recorded source snapshots; it does not establish a final synchronized build of the entire concurrently changing workspace.
