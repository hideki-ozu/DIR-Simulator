# DIR-Simulator project map

- This is a Rust discrete-event simulator project for network and SoC models. Requirements, scope, and interfaces are maintained in `docs/`; implementation details belong in the architecture/design docs.
- Current repository state: the Cargo workspace has no members and no simulator runtime yet. Do not infer runtime behavior from design documents.
- Invariants: implement NED behavior independently from public specifications; do not reuse OMNeT++ or INET implementation code. The target is a Rust-native API, not OMNeT++ C++/`.msg` or INET binary/API compatibility.
- Read `mem:tech_stack` for languages and tools, `mem:conventions` for document and diagram rules, and `mem:suggested_commands` for repository commands.