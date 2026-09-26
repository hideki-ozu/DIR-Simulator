# DIR Simulator

**DIR = Definition, Initialization, Runtime**

DIR Simulator is a Rust-based discrete-event simulator for network and SoC systems.  
The project adopts a three-layer model:

1. **Definition** — NED-compatible structural description
2. **Initialization** — scenario and parameter overrides via INI
3. **Runtime** — Rust-native simulation modules and execution engine

## Goals

- High compatibility with the useful structural subset of NED
- Rust-native runtime API rather than OMNeT++ C++ API compatibility
- A common simulation model for:
  - CAN / CAN-FD
  - Ethernet
  - gateways and switches
  - SoC buses / interconnects
  - NoC
  - DDR / SRAM
  - shared-memory IPC
  - DMA / mailbox IPC
- Explicit modeling of delay, buffers, shared resources, arbitration, and metrics
- Deterministic discrete-event execution

## Three-layer architecture

```text
Definition (NED)
      ↓
Initialization (INI)
      ↓
Runtime (Rust)
```

### Definition

NED describes system structure, module types, ports/gates, connections, and default parameters.

### Initialization

INI files describe simulation scenarios and override instance parameters.

### Runtime

Rust modules implement behavior. A Module Registry maps NED type names to Rust implementations.

## Compatibility policy

DIR is **not** intended to be a complete OMNeT++ replacement.

Initial direction:

- NED structural compatibility: targeted
- OMNeT++ C++ runtime API compatibility: not targeted
- `.msg` compatibility: not targeted
- INET binary/API compatibility: not targeted
- INET/NED import or mapping: possible future work

DIR is intended to be independently implemented from publicly documented language behavior rather than by porting OMNeT++ implementation code.

## Documentation

- [v0.1 Requirements](docs/requirements-v0.1.md)
- [Rust Architecture and API Design](docs/architecture.md)

## Status

Early design / bootstrap stage.

## License

Not selected yet.
