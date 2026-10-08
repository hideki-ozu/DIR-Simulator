//! Strict CAN FD control notifications; channel delay applies only to arrival.
use crate::types::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TX_SCHEMA: &str = "dir.canfd.TxRequestV1";
pub const RX_SCHEMA: &str = "dir.canfd.NotificationV1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Notification {
    pub kind: String,
    pub request_id: String,
    pub source: String,
    pub bus: String,
    pub receiver: Option<String>,
    pub frame_id: String,
    pub time_ps: u64,
    pub generation: u64,
}
// Field order is the required lexicographic order. Value makes receiver required
// during serde decoding, whereas an Option field would silently accept omission.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    bus: String,
    frame_id: String,
    generation: String,
    kind: String,
    receiver: Value,
    request_id: String,
    source: String,
    time_ps: String,
}
fn fail() -> Diagnostic {
    Diagnostic::execution("invalid CAN FD notification schema or ownership")
}
fn decimal(s: &str) -> Result<u64, Diagnostic> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(fail());
    }
    s.parse().map_err(|_| fail())
}
impl Notification {
    pub fn encode(&self) -> Result<String, Diagnostic> {
        self.validate()?;
        serde_json::to_string(&Body {
            bus: self.bus.clone(),
            frame_id: self.frame_id.clone(),
            generation: self.generation.to_string(),
            kind: self.kind.clone(),
            receiver: self
                .receiver
                .as_ref()
                .map_or(Value::Null, |s| Value::String(s.clone())),
            request_id: self.request_id.clone(),
            source: self.source.clone(),
            time_ps: self.time_ps.to_string(),
        })
        .map_err(|_| fail())
    }
    pub fn decode(content: &str, schema: &str) -> Result<Self, Diagnostic> {
        let body: Body = serde_json::from_str(content).map_err(|_| fail())?;
        let receiver = match body.receiver {
            Value::Null => None,
            Value::String(s) => Some(s),
            _ => return Err(fail()),
        };
        let notice = Self {
            kind: body.kind,
            request_id: body.request_id,
            source: body.source,
            bus: body.bus,
            receiver,
            frame_id: body.frame_id,
            time_ps: decimal(&body.time_ps)?,
            generation: decimal(&body.generation)?,
        };
        notice.validate()?;
        if schema
            != if notice.kind == "ready" {
                TX_SCHEMA
            } else {
                RX_SCHEMA
            }
        {
            return Err(fail());
        }
        Ok(notice)
    }
    fn validate(&self) -> Result<(), Diagnostic> {
        if self.request_id.is_empty()
            || self.source.is_empty()
            || self.bus.is_empty()
            || self.frame_id.is_empty()
            || match self.kind.as_str() {
                "ready" => self.receiver.is_some(),
                "arrival" => self.receiver.as_ref().is_none_or(String::is_empty),
                _ => true,
            }
        {
            return Err(fail());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_required_notification_schema_and_canonical_encoding() {
        let notice = Notification {
            kind: "ready".into(),
            request_id: "a:0".into(),
            source: "Main.a".into(),
            bus: "Main.bus".into(),
            receiver: None,
            frame_id: "a".into(),
            time_ps: 0,
            generation: 0,
        };
        let encoded = notice.encode().unwrap();
        assert_eq!(
            encoded,
            r#"{"bus":"Main.bus","frame_id":"a","generation":"0","kind":"ready","receiver":null,"request_id":"a:0","source":"Main.a","time_ps":"0"}"#
        );
        assert_eq!(Notification::decode(&encoded, TX_SCHEMA).unwrap(), notice);
        for invalid in [
            encoded.replace("\"receiver\":null,", ""),
            encoded.replace(
                "\"kind\":\"ready\",",
                "\"kind\":\"ready\",\"kind\":\"ready\",",
            ),
            encoded.replace("\"time_ps\":\"0\"", "\"time_ps\":\"00\""),
            encoded.replace("\"receiver\":null", "\"receiver\":\"Main.b\""),
            encoded.replace("\"time_ps\":\"0\"", "\"time_ps\":0"),
            encoded.replace("\"kind\":\"ready\"", "\"kind\":\"arrival\""),
        ] {
            assert_eq!(
                Notification::decode(&invalid, TX_SCHEMA).unwrap_err().code,
                "E-0002"
            );
        }
        assert!(Notification::decode(&encoded, RX_SCHEMA).is_err());
    }
}
