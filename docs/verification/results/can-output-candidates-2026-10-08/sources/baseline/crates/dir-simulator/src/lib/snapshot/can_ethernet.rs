//! Composite conversion, branch and frozen terminal opportunity records.
use crate::types::can_ethernet::CanFormat;
use serde::Serialize;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversionRecord {
    pub conversion_id: String,
    pub origin_id: String,
    pub parent_id: String,
    pub gateway: String,
    pub ingress_record: String,
    pub rule_id: Option<String>,
    pub visited_gateways: Vec<String>,
    pub observed_ps: u64,
    pub ready_ps: Option<u64>,
    pub planned_ready_ps: Option<u64>,
    pub released_ps: Option<u64>,
    pub status: String,
    pub reason: Option<String>,
    pub branch_ids: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchRecord {
    pub branch_id: String,
    pub conversion_id: String,
    pub egress: String,
    pub child_id: Option<String>,
    pub planned_child_id: String,
    pub codec_length: u64,
    pub pcp: Option<u8>,
    pub input_format: CanFormat,
    pub input_can_id: u32,
    pub output_format: CanFormat,
    pub output_can_id: u32,
    pub offer_ps: Option<u64>,
    pub planned_offer_ps: u64,
    pub admitted_ps: Option<u64>,
    pub sof_ps: Option<u64>,
    pub status: String,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SegmentTarget {
    pub origin_id: String,
    pub segment_id: String,
    pub branch_lineage: Vec<String>,
    pub terminal_id: String,
    pub completed_ps: Option<u64>,
    pub completion_reception_id: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SegmentRecord {
    pub segment_id: String,
    pub origin_id: String,
    pub source_record_id: String,
    pub branch_lineage: Vec<String>,
    pub sof_ps: u64,
    pub target_count: u64,
    pub targets: Vec<SegmentTarget>,
}
