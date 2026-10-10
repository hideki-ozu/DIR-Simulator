//! Private fixed transaction codecs; workload cannot supply payloads or generations.
use crate::types::Diagnostic;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Body {
    pub generation: String,
    pub kind: String,
    pub node: String,
    #[serde(deserialize_with = "required_optional")]
    pub request_id: Option<String>,
}
fn required_optional<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
impl Body {
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|e| invalid(e.to_string()))
    }
    fn validate(&self) -> Result<(), Diagnostic> {
        let g = &self.generation;
        if g.is_empty()
            || g.len() > 1 && g.starts_with('0')
            || !g.bytes().all(|b| b.is_ascii_digit())
            || g.parse::<u64>().is_err()
        {
            return Err(invalid("invalid transaction generation"));
        }
        if !matches!(
            self.kind.as_str(),
            "Offer"
                | "Complete"
                | "RefreshDue"
                | "RefreshEnd"
                | "DmaSetup"
                | "DmaResponse"
                | "DmaNotify"
                | "MailNotify"
        ) {
            return Err(invalid("unknown transaction kind"));
        }
        if matches!(self.kind.as_str(), "RefreshDue" | "RefreshEnd") != self.request_id.is_none() {
            return Err(invalid("transaction request reference mismatch"));
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8], schema: &str) -> Result<Self, Diagnostic> {
        let b: Self = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        b.validate()?;
        if schema != format!("dir.memory-ipc.transaction.{}", b.kind) {
            return Err(invalid("transaction schema/kind mismatch"));
        }
        Ok(b)
    }
}
fn invalid(message: impl Into<String>) -> Diagnostic {
    let mut d = Diagnostic::execution(message);
    d.details = Some(serde_json::json!({"kind":"invalid_event"}));
    d
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_codec_rejects_unknown_duplicate_and_nondecimal_fields() {
        let b = Body {
            generation: "0".into(),
            kind: "Offer".into(),
            node: "Main.sram".into(),
            request_id: Some("g:0".into()),
        };
        let bytes = b.encode().unwrap();
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"generation":"0","kind":"Offer","node":"Main.sram","request_id":"g:0"}"#
        );
        assert_eq!(
            Body::decode(&bytes, "dir.memory-ipc.transaction.Offer").unwrap(),
            b
        );
        assert!(Body::decode(&bytes, "dir.memory-ipc.transaction.Complete").is_err());
        for text in [
            r#"{"generation":"00","kind":"Offer","node":"Main.sram","request_id":"g:0"}"#,
            r#"{"generation":"0","kind":"Offer","node":"Main.sram","request_id":"g:0","extra":1}"#,
            r#"{"generation":"0","kind":"Offer","node":"Main.sram","request_id":"g:0","kind":"Offer"}"#,
            r#"{"generation":"0","kind":"RefreshDue","node":"Main.sram","request_id":"g:0"}"#,
        ] {
            assert!(Body::decode(text.as_bytes(), "dir.memory-ipc.transaction.Offer").is_err());
        }
    }
}
