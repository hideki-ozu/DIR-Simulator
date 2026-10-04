//! Model-independent input, scheduling and diagnostic contracts.
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Schedule {
    Explicit(Vec<u64>),
    Periodic {
        start: u64,
        phase: u64,
        period: u64,
        end: Option<u64>,
        count: Option<u64>,
    },
}
impl Schedule {
    pub fn time(&self, ordinal: u64) -> Option<u128> {
        match self {
            Self::Explicit(times) => usize::try_from(ordinal)
                .ok()
                .and_then(|i| times.get(i))
                .map(|&t| t as u128),
            Self::Periodic {
                start,
                phase,
                period,
                end,
                count,
            } => {
                if count.is_some_and(|n| ordinal >= n) {
                    return None;
                }
                let time = *start as u128 + *phase as u128 + ordinal as u128 * *period as u128;
                if end.is_some_and(|n| time >= n as u128) {
                    None
                } else {
                    Some(time)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InputSnapshot {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub schema_version: u32,
    pub code: String,
    pub stage: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
impl Diagnostic {
    pub fn prepare(message: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            code: "E-0001".into(),
            stage: "prepare".into(),
            message: message.into(),
            details: None,
        }
    }
    pub fn execution(message: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            code: "E-0002".into(),
            stage: "run".into(),
            message: message.into(),
            details: None,
        }
    }
    pub fn output(message: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            code: "E-0003".into(),
            stage: "output".into(),
            message: message.into(),
            details: None,
        }
    }
}
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Diagnostic {}

#[derive(Debug, Clone)]
pub struct PreparedCommon {
    pub profile: String,
    pub network: String,
    pub module_paths: Vec<String>,
    pub time_limit_ps: u64,
    pub metrics_window_ps: u64,
    pub max_events: u64,
    pub max_delta_cycles: u64,
    pub channel_count: usize,
    pub config_path: PathBuf,
    pub model_config_path: Option<PathBuf>,
    pub workload_path: Option<PathBuf>,
    pub inputs: Vec<InputSnapshot>,
}
