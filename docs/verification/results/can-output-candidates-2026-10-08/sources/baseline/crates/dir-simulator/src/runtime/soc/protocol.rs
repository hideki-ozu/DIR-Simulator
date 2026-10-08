//! Canonical version-one event payloads for the SoC profile family.
use crate::types::Diagnostic;
use serde::Deserialize;
use serde_json::json;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Generate {
        generator_id: String,
        ordinal: u64,
    },
    Complete {
        resource: String,
        request_id: String,
        hop: u64,
    },
    Wake {
        context: String,
    },
}
fn invalid() -> Diagnostic {
    let mut d = Diagnostic::execution("invalid SoC event payload");
    d.details = Some(json!({"reason":"invalid_event"}));
    d
}
fn decimal(s: &str) -> Result<u64, Diagnostic> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(invalid());
    }
    s.parse().map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    generator_id: String,
    ordinal: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Complete {
    resource: String,
    request_id: String,
    hop: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wake {
    context: String,
}
impl Event {
    pub fn encode(&self) -> String {
        match self {
            Self::Generate {
                generator_id,
                ordinal,
            } => json!({"generator_id":generator_id,"ordinal":ordinal.to_string()}),
            Self::Complete {
                resource,
                request_id,
                hop,
            } => json!({"resource":resource,"request_id":request_id,"hop":hop.to_string()}),
            Self::Wake { context } => json!({"context":context}),
        }
        .to_string()
    }
    pub fn decode(
        profile: &str,
        schema: &str,
        version: u32,
        phase: u8,
        payload: &str,
    ) -> Result<Self, Diagnostic> {
        if !matches!(
            profile,
            "soc.shared.v1" | "ahb.transaction.v1" | "noc.xy.v1"
        ) || version != 1
        {
            return Err(invalid());
        }
        let event = match schema.strip_prefix(&format!("{profile}.")) {
            Some("Generate") if phase == 1 => {
                let g: Generate = serde_json::from_str(payload).map_err(|_| invalid())?;
                Self::Generate {
                    generator_id: g.generator_id,
                    ordinal: decimal(&g.ordinal)?,
                }
            }
            Some("Complete") if phase == 0 => {
                let c: Complete = serde_json::from_str(payload).map_err(|_| invalid())?;
                Self::Complete {
                    resource: c.resource,
                    request_id: c.request_id,
                    hop: decimal(&c.hop)?,
                }
            }
            Some("Wake") if phase == 0 => {
                let w: Wake = serde_json::from_str(payload).map_err(|_| invalid())?;
                Self::Wake { context: w.context }
            }
            _ => return Err(invalid()),
        };
        Ok(event)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_exact_keys_canonical_decimal_and_phases() {
        let p = "noc.xy.v1";
        let e = Event::Generate {
            generator_id: "g".into(),
            ordinal: 1,
        };
        assert_eq!(e.encode(), r#"{"generator_id":"g","ordinal":"1"}"#);
        assert_eq!(
            Event::decode(p, &format!("{p}.Generate"), 1, 1, &e.encode()).unwrap(),
            e
        );
        for payload in [
            r#"{"generator_id":"g","ordinal":"01"}"#,
            r#"{"generator_id":"g","ordinal":1}"#,
            r#"{"generator_id":"g","ordinal":"1","extra":0}"#,
            r#"{"generator_id":"g","ordinal":"1","ordinal":"1"}"#,
        ] {
            assert_eq!(
                Event::decode(p, &format!("{p}.Generate"), 1, 1, payload)
                    .unwrap_err()
                    .details
                    .unwrap()["reason"],
                "invalid_event"
            );
        }
        assert!(Event::decode(p, &format!("{p}.Generate"), 1, 0, &e.encode()).is_err());
    }
}
