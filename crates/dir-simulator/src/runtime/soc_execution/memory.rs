use super::*;
use crate::types::transaction_fabric as backing;
pub(super) fn targets(c: &Config) -> Vec<backing::Target> {
    c.memory
        .iter()
        .map(|r| backing::Target {
            id: r.id.clone(),
            base: r.base as u64,
            size: r.size as u64,
            kind: backing::MemoryKind::Sram,
            read_latency_cycles: 0,
            write_response_cycles: 0,
            initial: r.initial.clone(),
        })
        .collect()
}
pub(super) fn native_targets(c: &Config) -> Vec<backing::Target> {
    let mut result = targets(c);
    // AMBA AHB subordinate address allocation is at least 1 KiB. Multiple
    // 64-byte apertures share that decoder; unused words still fault in Soc.
    let blocks: BTreeSet<u32> = c.mailboxes.iter().map(|m| m.base & !1023).collect();
    result.extend(blocks.into_iter().map(|base| backing::Target {
        id: format!("mailbox_decoder_{base}"),
        base: base as u64,
        size: 1024,
        kind: backing::MemoryKind::Sram,
        read_latency_cycles: 0,
        write_response_cycles: 0,
        initial: vec![],
    }));
    result
}
impl Soc {
    pub(super) fn read_backing(
        &self,
        s: &State,
        txn: usize,
        address: u32,
        length: usize,
    ) -> ModelResult<Vec<u8>> {
        let zeros = vec![0; length];
        let (source, id) = s.transactions.get(txn).map_or(("soc", "writeback"), |t| {
            (t.request.source.as_str(), t.request.id.as_str())
        });
        Ok(s.memory
            .read(&MemoryRequest {
                source,
                request_id: id,
                id: 0,
                address: address as u64,
                data: &zeros,
                byte_enables: &[],
            })?
            .data)
    }
    pub(super) fn write_backing(
        &self,
        s: &mut State,
        txn: Option<usize>,
        address: u32,
        data: &[u8],
        enables: &[bool],
    ) -> ModelResult {
        let (source, id) = txn
            .and_then(|i| s.transactions.get(i))
            .map_or(("soc".to_string(), "writeback".to_string()), |t| {
                (t.request.source.clone(), t.request.id.clone())
            });
        s.memory.write(&MemoryRequest {
            source: &source,
            request_id: &id,
            id: 0,
            address: address as u64,
            data,
            byte_enables: enables,
        })?;
        Ok(())
    }
    pub(super) fn atomic(&self, request: &Request, old: u32) -> ModelResult<u32> {
        let a = request
            .atomic
            .as_ref()
            .ok_or("missing AXI5 atomic opcode")?;
        let word = |start: usize| {
            u32::from_le_bytes(
                request.data[start..start + 4]
                    .try_into()
                    .expect("validated operand shape"),
            )
        };
        if a.awatop == 0x31 {
            let (compare, swap) = if request.address % 8 == 0 {
                (word(0), word(4))
            } else {
                (word(4), word(0))
            };
            return Ok(if old == compare { swap } else { old });
        }
        let operand = word(0);
        Ok(match a.awatop {
            0x30 => operand,
            _ => match a.awatop & 7 {
                0 => old.wrapping_add(operand),
                1 => old & !operand,
                2 => old ^ operand,
                3 => old | operand,
                4 => (old as i32).max(operand as i32) as u32,
                5 => (old as i32).min(operand as i32) as u32,
                6 => old.max(operand),
                7 => old.min(operand),
                _ => unreachable!(),
            },
        })
    }
}
