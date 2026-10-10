use super::{EditorError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Position {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl Position {
    pub fn new(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            collapsed: false,
            extra: BTreeMap::new(),
        }
    }
    pub fn check(&self) -> Result<()> {
        if !self.x.is_finite()
            || !self.y.is_finite()
            || self.x.abs() > 1_000_000.0
            || self.y.abs() > 1_000_000.0
        {
            return Err(EditorError::new(
                "E-EDITOR-LAYOUT",
                "Coordinates must be finite and within ±1,000,000",
                422,
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TypeLayout {
    pub nodes: BTreeMap<String, Position>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct LayoutDocument {
    pub schema_version: u32,
    pub source_file: String,
    pub types: BTreeMap<String, TypeLayout>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl LayoutDocument {
    pub fn empty(source_file: String) -> Self {
        Self {
            schema_version: 1,
            source_file,
            types: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
    pub fn parse(text: &str) -> Result<Self> {
        let value = super::controller::strict_json(text.as_bytes())?;
        let layout: Self = serde_json::from_value(value)
            .map_err(|e| EditorError::new("E-EDITOR-LAYOUT", e.to_string(), 422))?;
        if layout.schema_version != 1 {
            return Err(EditorError::new(
                "E-EDITOR-LAYOUT",
                "Unsupported layout schema",
                422,
            ));
        }
        for ty in layout.types.values() {
            for position in ty.nodes.values() {
                position.check()?;
            }
        }
        Ok(layout)
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| EditorError::new("E-EDITOR-LAYOUT", e.to_string(), 500))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
