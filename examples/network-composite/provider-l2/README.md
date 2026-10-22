# Provider/customer component production fixture

Two PEs carry separate services S-VID 100 and 200. Both customers use C-VID 10 and deliberately reuse source/destination MAC addresses. East customer A has three physical customer edge ports; PEP/CNP identifiers describe internal handoff and are not NED wires.

The composite owner executes this fixture through physical output queues and logical C/S handoffs. The data-path run observed six intended deliveries, six PNP transmissions and twelve internal handoffs. Protocol-enabled variants and remaining adversarial checks are tracked separately below.

Required observations:

- Learning and forwarding stay within `(component, VID, MAC)` even for reused addresses. `a_data`, `a1_data`, `a2_data` reach a3; `b_data` reaches b1, with no cross-service deliveries.
- Every PNP transmission contains outer TPID 0x88a8 and S-VID 100 or 200 followed by unchanged customer TPID 0x8100, VID 10, PCP/DEI and payload. Customer egress restores the C-tag and data. FCS and MAC lengths derive from actual bytes.
- Internal C/S handoff creates no physical transmission. All real egresses consume ordinary finite output queues, serialization and IFG.
- Three east A customer ports remain accepted and use shared PEP rules for control traffic; this fixture alone does not verify those rules. A protocol-enabled variant must test customer BPDU forwarding, provider BPDU consumption, component-specific tree states and scoped FDB flush.
- A checkpoint during queued and active service traffic resumes to identical actual output; rehashed component/VID/tag corruption rejects.
