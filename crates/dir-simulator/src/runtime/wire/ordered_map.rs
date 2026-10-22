//! Canonical entry-array serialization for maps with structured keys.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

pub fn serialize<K: Serialize + Ord, V: Serialize, S: Serializer>(
    value: &BTreeMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.iter().collect::<Vec<_>>().serialize(serializer)
}
pub fn deserialize<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<K, V>, D::Error> {
    let entries = Vec::<(K, V)>::deserialize(deserializer)?;
    if entries.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(serde::de::Error::custom("unordered or duplicate map key"));
    }
    Ok(entries.into_iter().collect())
}
