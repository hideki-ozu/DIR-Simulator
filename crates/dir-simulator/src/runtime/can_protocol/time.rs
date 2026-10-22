//! Checked exact local time; rounding is confined to absolute scheduler publication.
use crate::registry::{ModelError, ModelResult};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Exact {
    #[serde(with = "decimal")]
    pub num: u128,
    #[serde(with = "decimal")]
    pub den: u128,
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
impl Exact {
    pub fn new(num: u128, den: u128) -> ModelResult<Self> {
        if den == 0 {
            return Err("zero exact-time denominator".into());
        }
        let d = gcd(num, den);
        Ok(Self {
            num: num / d,
            den: den / d,
        })
    }
    pub fn integer(num: u64) -> Self {
        Self {
            num: num as u128,
            den: 1,
        }
    }
    pub fn add(self, other: Self) -> ModelResult<Self> {
        let d = gcd(self.den, other.den);
        let a = other.den / d;
        let b = self.den / d;
        Self::new(
            self.num
                .checked_mul(a)
                .and_then(|n| other.num.checked_mul(b).and_then(|m| n.checked_add(m)))
                .ok_or("exact CAN time overflow")?,
            self.den
                .checked_mul(a)
                .ok_or("exact CAN denominator overflow")?,
        )
    }
    pub fn fraction(self, num: u64, den: u64) -> ModelResult<Self> {
        let a = Self::new(num as u128, den as u128)?;
        let d = gcd(self.num, a.den);
        let e = gcd(a.num, self.den);
        Self::new(
            (self.num / d)
                .checked_mul(a.num / e)
                .ok_or("exact CAN fraction overflow")?,
            (self.den / e)
                .checked_mul(a.den / d)
                .ok_or("exact CAN fraction denominator overflow")?,
        )
    }
    pub fn compare(self, other: Self) -> ModelResult<std::cmp::Ordering> {
        let d = gcd(self.den, other.den);
        Ok(self
            .num
            .checked_mul(other.den / d)
            .ok_or("exact CAN comparison overflow")?
            .cmp(
                &other
                    .num
                    .checked_mul(self.den / d)
                    .ok_or("exact CAN comparison overflow")?,
            ))
    }
    pub fn ceil(self) -> ModelResult<u64> {
        u64::try_from(self.num.div_ceil(self.den))
            .map_err(|_| ModelError("CAN scheduler time overflow".into()))
    }
    pub fn valid(self) -> bool {
        self.den != 0 && gcd(self.num, self.den) == 1
    }
}
mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        let v: u128 = s.parse().map_err(serde::de::Error::custom)?;
        if s != v.to_string() {
            return Err(serde::de::Error::custom("noncanonical exact-time decimal"));
        }
        Ok(v)
    }
}
