//! One byte-memory owner shared by every AXI and direct/DMA request in a fabric.
use crate::registry::ModelResult;
use crate::types::transaction_fabric::{Operation, Request, Target};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryService {
    pub regions: Vec<MemoryRegion>,
    monitors: Vec<Monitor>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRegion {
    pub id: String,
    pub base: u64,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Monitor {
    source: String,
    request_id: String,
    id: u32,
    address: u64,
    bytes: u64,
    completed_read: bool,
}
#[derive(Clone, Debug)]
pub struct MemoryRequest<'a> {
    pub source: &'a str,
    pub request_id: &'a str,
    pub id: u32,
    pub address: u64,
    pub data: &'a [u8],
    pub byte_enables: &'a [bool],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryResponse {
    pub request_id: String,
    pub source: String,
    pub id: u32,
    pub data: Vec<u8>,
}
impl MemoryService {
    pub fn new(targets: &[Target]) -> Self {
        Self {
            regions: targets
                .iter()
                .map(|t| {
                    let mut bytes = vec![0; t.size as usize];
                    bytes[..t.initial.len()].copy_from_slice(&t.initial);
                    MemoryRegion {
                        id: t.id.clone(),
                        base: t.base,
                        bytes,
                    }
                })
                .collect(),
            monitors: Vec::new(),
        }
    }
    pub fn read(&self, request: &MemoryRequest<'_>) -> ModelResult<MemoryResponse> {
        let mut data = Vec::with_capacity(request.data.len());
        for i in 0..request.data.len() {
            let address = request
                .address
                .checked_add(i as u64)
                .ok_or("memory address overflow")?;
            let (region, offset) = self.locate(address).ok_or("memory decode error")?;
            data.push(self.regions[region].bytes[offset]);
        }
        Ok(MemoryResponse {
            request_id: request.request_id.into(),
            source: request.source.into(),
            id: request.id,
            data,
        })
    }
    /// The single visibility point; preflight all addresses before changing any byte.
    pub fn write(&mut self, request: &MemoryRequest<'_>) -> ModelResult<MemoryResponse> {
        if request.data.len() != request.byte_enables.len() {
            return Err("memory enable width mismatch".into());
        }
        let mut writes = Vec::new();
        for (i, enabled) in request.byte_enables.iter().enumerate() {
            if *enabled {
                let address = request
                    .address
                    .checked_add(i as u64)
                    .ok_or("memory address overflow")?;
                let (region, offset) = self.locate(address).ok_or("memory decode error")?;
                writes.push((region, offset, address, request.data[i]));
            }
        }
        for (region, offset, address, byte) in writes {
            self.regions[region].bytes[offset] = byte;
            self.monitors
                .retain(|m| !(address >= m.address && address - m.address < m.bytes));
        }
        Ok(MemoryResponse {
            request_id: request.request_id.into(),
            source: request.source.into(),
            id: request.id,
            data: vec![],
        })
    }
    pub fn begin_exclusive_read(
        &mut self,
        source: &str,
        id: u32,
        request_id: &str,
        address: u64,
        bytes: u64,
    ) {
        // A new exclusive read replaces this manager/ID monitor.
        self.monitors.retain(|m| m.source != source || m.id != id);
        self.monitors.push(Monitor {
            source: source.into(),
            request_id: request_id.into(),
            id,
            address,
            bytes,
            completed_read: false,
        });
    }
    /// Grant only after the actual last R handshake; an intervening writer removes the candidate.
    pub fn complete_exclusive_read(
        &mut self,
        source: &str,
        id: u32,
        request_id: &str,
        address: u64,
        bytes: u64,
    ) {
        if let Some(m) = self.monitors.iter_mut().find(|m| {
            m.source == source
                && m.id == id
                && m.request_id == request_id
                && m.address == address
                && m.bytes == bytes
        }) {
            m.completed_read = true;
        }
    }
    pub fn take_exclusive(&mut self, source: &str, id: u32, address: u64, bytes: u64) -> bool {
        let matched = self.monitors.iter().any(|m| {
            m.source == source
                && m.id == id
                && m.address == address
                && m.bytes == bytes
                && m.completed_read
        });
        self.monitors.retain(|m| m.source != source || m.id != id);
        matched
    }
    fn locate(&self, address: u64) -> Option<(usize, usize)> {
        self.regions.iter().enumerate().find_map(|(i, r)| {
            address
                .checked_sub(r.base)
                .filter(|o| *o < r.bytes.len() as u64)
                .map(|o| (i, o as usize))
        })
    }
    pub fn validate(&self, targets: &[Target]) -> ModelResult {
        if self.regions.len() != targets.len()
            || self
                .regions
                .iter()
                .zip(targets)
                .any(|(r, t)| r.id != t.id || r.base != t.base || r.bytes.len() as u64 != t.size)
        {
            return Err("checkpoint memory map mismatch".into());
        }
        let mut ids = BTreeMap::new();
        for m in &self.monitors {
            if m.bytes == 0
                || !m.bytes.is_power_of_two()
                || m.bytes > 128
                || m.address % m.bytes != 0
                || self.locate(m.address).is_none()
                || m.address
                    .checked_add(m.bytes - 1)
                    .and_then(|end| self.locate(end))
                    .is_none()
                || ids.insert((&m.source, m.id), ()).is_some()
            {
                return Err("invalid checkpoint exclusive monitor".into());
            }
        }
        Ok(())
    }
    pub(crate) fn validate_monitors(
        &self,
        requests: &[Request],
        completed: impl Fn(usize) -> bool,
    ) -> ModelResult {
        for m in &self.monitors {
            let Some((index, r)) = requests
                .iter()
                .enumerate()
                .find(|(_, r)| r.request_id == m.request_id)
            else {
                return Err("unknown checkpoint exclusive read reference".into());
            };
            if r.operation != Operation::Read
                || !r.exclusive
                || r.source != m.source
                || r.id != m.id
                || r.address != m.address
                || (r.beats * r.size_bytes) as u64 != m.bytes
                || completed(index) != m.completed_read
            {
                return Err("checkpoint exclusive read identity/completion mismatch".into());
            }
        }
        Ok(())
    }
}
