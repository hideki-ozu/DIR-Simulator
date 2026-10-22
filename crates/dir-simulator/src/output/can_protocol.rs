//! Versioned CAN protocol observation schema validation.
use crate::registry::ModelResult;
use serde_json::Value;
pub fn validate_record(v: &Value) -> ModelResult {
    if v.is_object()
        && matches!(
            v.get("kind").and_then(Value::as_str),
            Some(
                "local_sof"
                    | "cancellation"
                    | "application"
                    | "generator"
                    | "controller"
                    | "request"
                    | "attempt"
                    | "error"
                    | "reception"
                    | "overload"
            )
        )
    {
        Ok(())
    } else {
        Err("invalid CAN protocol observation".into())
    }
}

/// Compound records retain explicit actual/planned distinctions and exact clock identity.
pub fn validate_compound_record(value: &Value) -> ModelResult {
    use crate::registry::ModelError;
    let object = value
        .as_object()
        .ok_or("compound CAN record must be an object")?;
    if value["execution_contract"] != "ideal-digital-can.v1" {
        return Err("compound CAN record execution contract mismatch".into());
    }
    let kind = value["kind"]
        .as_str()
        .ok_or("compound CAN record kind absent")?;
    if !matches!(
        kind,
        "controller"
            | "request"
            | "attempt"
            | "error"
            | "reception"
            | "sample"
            | "local_sof"
            | "overload"
            | "route_update"
            | "gateway_ingress"
            | "gateway_branch"
            | "cancellation"
    ) {
        return Err("unknown compound CAN record kind".into());
    }
    for (key, data) in object {
        if key.ends_with("_ps") && !data.is_null() {
            let text = data
                .as_str()
                .ok_or("CAN time must be canonical decimal text")?;
            let number: u64 = text
                .parse()
                .map_err(|_| ModelError("invalid CAN time".into()))?;
            if text != number.to_string() {
                return Err("noncanonical CAN time".into());
            }
        }
        if (key.ends_with("_num") || key.ends_with("_den")) && !data.is_null() {
            let text = data.as_str().ok_or("exact CAN time must be decimal text")?;
            let number: u128 = text
                .parse()
                .map_err(|_| ModelError("invalid exact CAN time".into()))?;
            if text != number.to_string() || key.ends_with("_den") && number == 0 {
                return Err("noncanonical/zero-denominator exact CAN time".into());
            }
        }
    }
    if matches!(
        kind,
        "request" | "attempt" | "gateway_branch" | "cancellation"
    ) && value["request_id"].as_str().is_none_or(str::is_empty)
    {
        return Err("CAN record request identity missing".into());
    }
    if kind == "attempt"
        && (value["attempt"].as_u64().is_none_or(|n| n == 0)
            || !matches!(
                value["state"].as_str(),
                Some("transmitting" | "arbitration_lost" | "failed" | "completed")
            ))
    {
        return Err("CAN attempt ordinal/state invalid".into());
    }
    for key in ["wire_bits", "planned_wire_bits"] {
        if let Some(bits) = value.get(key) {
            if bits.as_array().is_none_or(|bits| {
                bits.len() > 1_000_000 || bits.iter().any(|b| b.as_u64().is_none_or(|n| n > 1))
            }) {
                return Err("CAN bit arena invalid".into());
            }
        }
    }
    if kind == "sample"
        && (value["sample"].as_u64().is_none_or(|n| n > 1)
            || value["resolved_level"].as_u64().is_none_or(|n| n > 1)
            || value["exact_num"].as_str().is_none()
            || value["exact_den"].as_str().is_none()
            || !matches!(value["rate"].as_str(), Some("nominal" | "data")))
    {
        return Err("CAN sampled clock/level record invalid".into());
    }
    if kind == "controller"
        && (value["tec"].as_u64().is_none_or(|n| n > 256)
            || value["rec"].as_u64().is_none_or(|n| n > 255)
            || !matches!(
                value["state"].as_str(),
                Some("error_active" | "error_passive" | "bus_off")
            ))
    {
        return Err("CAN controller confinement record invalid".into());
    }
    if kind == "reception" {
        let frame: crate::runtime::canfd::receiver::DecodedFrame =
            serde_json::from_value(value["frame"].clone())
                .map_err(|e| ModelError(e.to_string()))?;
        match frame {
            crate::runtime::canfd::receiver::DecodedFrame::Cc {
                id,
                extended,
                remote,
                dlc,
                data,
            } => {
                crate::runtime::can_protocol::codec::encode(
                    &crate::runtime::can_protocol::types::Frame {
                        id,
                        extended,
                        remote,
                        dlc,
                        data,
                    },
                )?;
            }
            crate::runtime::canfd::receiver::DecodedFrame::IsoFd {
                id,
                extended,
                brs,
                esi,
                data,
            } => {
                crate::runtime::canfd::codec::encode(&crate::runtime::canfd::codec::Frame {
                    id,
                    extended,
                    brs,
                    esi,
                    data,
                })
                .map_err(|e| ModelError(e.to_string()))?;
            }
        }
    } else {
        for key in ["frame", "source_frame"] {
            if let Some(frame) = value.get(key) {
                let frame: crate::runtime::can_protocol::types::ProtocolFrame =
                    serde_json::from_value(frame.clone()).map_err(|e| ModelError(e.to_string()))?;
                crate::runtime::can_protocol::input::validate_protocol_frame(&frame)?;
            }
        }
    }
    Ok(())
}
