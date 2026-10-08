//! Committed bytes, owners, requests and independent notification lifetimes.
use crate::types::memory_ipc::Request;
use std::collections::VecDeque;
#[derive(Debug, Clone)]
pub struct RequestRow {
    pub id: String,
    pub node: usize,
    pub generator: usize,
    pub ordinal: u64,
    pub origin: Option<usize>,
    pub chunk: u64,
    pub spec: Request,
    pub time: u64,
    pub generated: u64,
    pub started: Option<u64>,
    pub planned: Option<u64>,
    pub completed: Option<u64>,
    pub status: String,
    pub reason: Option<String>,
    pub output: Option<Vec<u8>>,
    pub slot: Option<usize>,
    pub port: Option<usize>,
    pub dispatch: Option<u64>,
    pub bank: Option<usize>,
    pub row: Option<u64>,
    pub row_hit: Option<bool>,
    pub message: Option<String>,
    pub committed: u64,
    pub data_done: Option<u64>,
    pub planned_notify: Option<u64>,
    pub notified: Option<u64>,
    pub response_consumed: bool,
}
#[derive(Debug, Clone)]
pub struct Slot {
    pub state: String,
    pub owner: Option<String>,
    pub message: Option<String>,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub bytes: Vec<u8>,
    pub enqueued: u64,
}
#[derive(Debug, Clone)]
pub struct Notification {
    pub node: usize,
    pub id: String,
    pub enqueued: u64,
    pub planned: u64,
    pub delivered: Option<u64>,
}
#[derive(Debug, Clone, Default)]
pub struct ResourceState {
    pub time: u64,
    pub bytes: Vec<u8>,
    pub open_rows: Vec<Option<u64>>,
    pub ports: Vec<Option<usize>>,
    pub queue: VecDeque<usize>,
    pub active: Option<usize>,
    pub refresh_pending: bool,
    pub refresh_started: Option<u64>,
    pub refresh_planned_end: Option<u64>,
    pub refresh_ended: Option<u64>,
    pub refreshes: u64,
    pub slots: Vec<Slot>,
    pub ready: VecDeque<usize>,
    pub messages: VecDeque<Message>,
    pub child: Option<usize>,
    pub chunk_index: u64,
    pub chunk_hex: Option<Vec<u8>>,
    pub next_dispatch: u64,
}
#[derive(Debug, Clone)]
pub struct QueuePoint {
    pub node: usize,
    pub time: u64,
    pub value: usize,
    pub request: Option<String>,
    pub event: u64,
}
#[derive(Debug, Clone, Default)]
pub struct MemoryIpcSnapshot {
    pub requests: Vec<RequestRow>,
    pub resources: Vec<ResourceState>,
    pub notifications: Vec<Notification>,
    pub queues: Vec<QueuePoint>,
}
