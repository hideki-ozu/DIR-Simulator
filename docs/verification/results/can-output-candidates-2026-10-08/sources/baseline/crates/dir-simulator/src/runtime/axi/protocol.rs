//! Version-one AXI payload codecs; required keys and duplicate keys are strict.
use crate::types::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Address {
    pub address: String,
    pub burst: u8,
    pub id: u8,
    pub len: String,
    pub manager: String,
    pub request_id: String,
    pub size: u8,
    pub target: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub beat: String,
    pub data_hex: String,
    pub last: bool,
    pub request_id: String,
    pub strb: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub id: u8,
    pub request_id: String,
    pub resp: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub beat: String,
    pub data_hex: String,
    pub id: u8,
    pub last: bool,
    pub request_id: String,
    pub resp: String,
}
fn fail() -> Diagnostic {
    let mut d = Diagnostic::execution("invalid AXI payload schema");
    d.details = Some(json!({"reason":"invalid_event"}));
    d
}
fn decimal(s: &str, max: u64) -> Result<u64, Diagnostic> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(fail());
    }
    s.parse::<u64>().ok().filter(|n| *n <= max).ok_or_else(fail)
}
fn data(s: &str) -> bool {
    s.len() == 8
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn response(s: &str) -> bool {
    matches!(s, "OKAY" | "SLVERR" | "DECERR")
}
pub fn decode(content: &str, schema: &str) -> Result<Value, Diagnostic> {
    let channel = schema
        .strip_prefix("axi4.transaction.v1.")
        .ok_or_else(fail)?;
    match channel {
        "Aw" | "Ar" => {
            let b: Address = serde_json::from_str(content).map_err(|_| fail())?;
            let address = decimal(&b.address, u32::MAX as u64)?;
            let len = decimal(&b.len, 255)?;
            if b.burst != 1
                || b.id != 0
                || b.size != 2
                || b.manager.is_empty()
                || b.target.is_empty()
                || b.request_id.is_empty()
                || address % 4 != 0
                || address + 4 * (len + 1) > 1u64 << 32
                || address / 4096 != (address + 4 * (len + 1) - 1) / 4096
            {
                return Err(fail());
            }
            serde_json::to_value(b).map_err(|_| fail())
        }
        "W" => {
            let b: Write = serde_json::from_str(content).map_err(|_| fail())?;
            decimal(&b.beat, 255)?;
            decimal(&b.strb, 15)?;
            if !data(&b.data_hex) || b.request_id.is_empty() {
                return Err(fail());
            }
            serde_json::to_value(b).map_err(|_| fail())
        }
        "B" => {
            let b: Response = serde_json::from_str(content).map_err(|_| fail())?;
            if b.id != 0 || b.request_id.is_empty() || !response(&b.resp) {
                return Err(fail());
            }
            serde_json::to_value(b).map_err(|_| fail())
        }
        "R" => {
            let b: Read = serde_json::from_str(content).map_err(|_| fail())?;
            decimal(&b.beat, 255)?;
            if b.id != 0 || b.request_id.is_empty() || !response(&b.resp) || !data(&b.data_hex) {
                return Err(fail());
            }
            serde_json::to_value(b).map_err(|_| fail())
        }
        _ => Err(fail()),
    }
}
pub fn encode(payload: &Value, schema: &str) -> Result<String, Diagnostic> {
    let bytes = serde_json::to_string(payload).map_err(|_| fail())?;
    decode(&bytes, schema)?;
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_payload_keys_types_and_canonical_decimal() {
        let good =
            r#"{"beat":"0","data_hex":"11223344","last":true,"request_id":"a:0","strb":"5"}"#;
        assert!(decode(good, "axi4.transaction.v1.W").is_ok());
        for bad in [
            good.replace("\"beat\":\"0\"", "\"beat\":\"00\""),
            good.replace("\"last\":true,", ""),
            good.replace("\"last\":true", "\"last\":true,\"last\":true"),
            good.replace("\"strb\":\"5\"", "\"strb\":5"),
            good.replace("11223344", "AABBCCDD"),
        ] {
            assert_eq!(
                decode(&bad, "axi4.transaction.v1.W").unwrap_err().code,
                "E-0002"
            );
        }
        assert!(decode(good, "axi4.transaction.v1.R").is_err());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Internal {
    Generate {
        generator_id: String,
        ordinal: u64,
    },
    Wake {
        interconnect: String,
    },
    Handshake {
        request_id: String,
        channel: String,
        beat: Option<u64>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateBody {
    generator_id: String,
    ordinal: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WakeBody {
    interconnect: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandshakeBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    beat: Option<String>,
    channel: String,
    request_id: String,
}
impl Internal {
    pub fn schema(&self) -> &'static str {
        match self {
            Self::Generate { .. } => "axi4.transaction.v1.Generate",
            Self::Wake { .. } => "axi4.transaction.v1.Wake",
            Self::Handshake { .. } => "axi4.transaction.v1.Handshake",
        }
    }
    pub fn encode(&self) -> Result<String, Diagnostic> {
        let text = match self {
            Self::Generate {
                generator_id,
                ordinal,
            } => serde_json::to_string(&GenerateBody {
                generator_id: generator_id.clone(),
                ordinal: ordinal.to_string(),
            }),
            Self::Wake { interconnect } => serde_json::to_string(&WakeBody {
                interconnect: interconnect.clone(),
            }),
            Self::Handshake {
                request_id,
                channel,
                beat,
            } => serde_json::to_string(&HandshakeBody {
                request_id: request_id.clone(),
                channel: channel.clone(),
                beat: beat.map(|n| n.to_string()),
            }),
        }
        .map_err(|_| fail())?;
        Self::decode(&text, self.schema())?;
        Ok(text)
    }
    pub fn decode(text: &str, schema: &str) -> Result<Self, Diagnostic> {
        match schema {
            "axi4.transaction.v1.Generate" => {
                let b: GenerateBody = serde_json::from_str(text).map_err(|_| fail())?;
                if b.generator_id.is_empty() {
                    return Err(fail());
                }
                Ok(Self::Generate {
                    generator_id: b.generator_id,
                    ordinal: decimal(&b.ordinal, u64::MAX)?,
                })
            }
            "axi4.transaction.v1.Wake" => {
                let b: WakeBody = serde_json::from_str(text).map_err(|_| fail())?;
                if b.interconnect.is_empty() {
                    return Err(fail());
                }
                Ok(Self::Wake {
                    interconnect: b.interconnect,
                })
            }
            "axi4.transaction.v1.Handshake" => {
                let b: HandshakeBody = serde_json::from_str(text).map_err(|_| fail())?;
                if b.request_id.is_empty()
                    || !matches!(b.channel.as_str(), "AW" | "W" | "B" | "AR" | "R")
                    || matches!(b.channel.as_str(), "W" | "R") != b.beat.is_some()
                {
                    return Err(fail());
                }
                let beat = b.beat.map(|s| decimal(&s, 255)).transpose()?;
                Ok(Self::Handshake {
                    request_id: b.request_id,
                    channel: b.channel,
                    beat,
                })
            }
            _ => Err(fail()),
        }
    }
}
