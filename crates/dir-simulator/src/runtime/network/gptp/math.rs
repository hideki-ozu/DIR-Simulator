//! Checked reduced-rational timestamp arithmetic; protocol observations never
//! consult another node's configured oscillator law.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rat {
    #[serde(with = "decimal_i128")]
    pub num: i128,
    #[serde(with = "decimal_i128")]
    pub den: i128,
}
mod decimal_i128 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &i128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i128, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || value.starts_with('+')
            || value.starts_with('0') && value != "0"
            || value.starts_with("-0")
        {
            return Err(serde::de::Error::custom("noncanonical decimal i128"));
        }
        value.parse::<i128>().map_err(serde::de::Error::custom)
    }
}
impl Rat {
    pub const ZERO: Self = Self { num: 0, den: 1 };
    pub const ONE: Self = Self { num: 1, den: 1 };
    pub fn new(num: i128, den: i128) -> Result<Self, &'static str> {
        if den <= 0 {
            return Err("nonpositive rational denominator");
        }
        let g = gcd(num.unsigned_abs(), den as u128) as i128;
        Ok(Self {
            num: num / g,
            den: den / g,
        })
    }
    pub fn add(self, rhs: Self) -> Result<Self, &'static str> {
        let g = gcd(self.den as u128, rhs.den as u128) as i128;
        let a = self.den / g;
        let b = rhs.den / g;
        Self::new(
            self.num
                .checked_mul(b)
                .and_then(|n| rhs.num.checked_mul(a).and_then(|m| n.checked_add(m)))
                .ok_or("rational addition overflow")?,
            a.checked_mul(rhs.den)
                .ok_or("rational denominator overflow")?,
        )
    }
    pub fn sub(self, rhs: Self) -> Result<Self, &'static str> {
        self.add(rhs.neg()?)
    }
    pub fn neg(self) -> Result<Self, &'static str> {
        Self::new(
            self.num.checked_neg().ok_or("rational negation overflow")?,
            self.den,
        )
    }
    pub fn mul(self, rhs: Self) -> Result<Self, &'static str> {
        let a = gcd(self.num.unsigned_abs(), rhs.den as u128) as i128;
        let b = gcd(rhs.num.unsigned_abs(), self.den as u128) as i128;
        Self::new(
            (self.num / a)
                .checked_mul(rhs.num / b)
                .ok_or("rational multiplication overflow")?,
            (self.den / b)
                .checked_mul(rhs.den / a)
                .ok_or("rational denominator overflow")?,
        )
    }
    pub fn div(self, rhs: Self) -> Result<Self, &'static str> {
        if rhs.num == 0 {
            return Err("rational division by zero");
        }
        let sign: i128 = if rhs.num < 0 { -1 } else { 1 };
        self.mul(Self::new(
            sign.checked_mul(rhs.den)
                .ok_or("rational division overflow")?,
            rhs.num
                .checked_mul(sign)
                .ok_or("rational division overflow")?,
        )?)
    }
    pub fn half(self) -> Result<Self, &'static str> {
        self.div(Self::new(2, 1)?)
    }
    pub fn floor(self) -> i128 {
        self.num.div_euclid(self.den)
    }
    pub fn ceil(self) -> Result<i128, &'static str> {
        let floor = self.floor();
        if self.num.rem_euclid(self.den) == 0 {
            Ok(floor)
        } else {
            floor.checked_add(1).ok_or("rational ceiling overflow")
        }
    }
    pub fn positive(self) -> bool {
        self.num > 0
    }
    pub fn within_integer_threshold(self, threshold: u64) -> bool {
        if self.num < 0 {
            return false;
        }
        let whole = self.num / self.den;
        let limit = i128::from(threshold);
        whole < limit || (whole == limit && self.num % self.den == 0)
    }
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let c = a % b;
        a = b;
        b = c;
    }
    a
}

pub fn raw(
    offset_ps: i128,
    rate_num: u64,
    rate_den: u64,
    global_ps: u64,
) -> Result<Rat, &'static str> {
    Rat::new(offset_ps, 1)?.add(Rat::new(
        i128::from(global_ps)
            .checked_mul(i128::from(rate_num))
            .ok_or("raw oscillator overflow")?,
        i128::from(rate_den),
    )?)
}

pub fn pdelay(
    previous: Option<(Rat, Rat)>,
    t1: Rat,
    t2: Rat,
    t3: Rat,
    t4: Rat,
) -> Result<Option<(Rat, Rat)>, &'static str> {
    let Some((old_t3, old_t4)) = previous else {
        return Ok(None);
    };
    let n = t3.sub(old_t3)?;
    let d = t4.sub(old_t4)?;
    if !n.positive() || !d.positive() {
        return Err("nonpositive Pdelay sample interval");
    }
    let r = n.div(d)?;
    let delay = r.mul(t4.sub(t1)?)?.sub(t3.sub(t2)?)?.half()?;
    if delay.num < 0 {
        return Err("negative Pdelay");
    }
    Ok(Some((r, delay)))
}

/// Open wire-quantization enclosures. Delay and quantum are in the neighbor's
/// raw-clock picoseconds. The selected rate/delay are estimates, not a claim
/// that their pair is one jointly feasible exact history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PdelayQuantization {
    pub delay_units: PdelayDelayUnits,
    pub point_delay: Rat,
    pub wire_quantum: Rat,
    pub delay_error_bound: Rat,
    pub delay_lower_open: Rat,
    pub delay_upper_open: Rat,
    pub rate_lower_open: Rat,
    pub rate_upper_open: Rat,
    pub quantization_limited_zero: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdelayDelayUnits {
    NeighborRawClockPicoseconds,
}

/// Selected sampled producer: timestamp plus floored fractional correction.
/// Static return propagation and immutable affine raw clocks are required.
pub fn pdelay_wire_quantized(
    previous: Option<(Rat, Rat)>,
    t1: Rat,
    t2: Rat,
    t3: Rat,
    t4: Rat,
) -> Result<Option<(Rat, Rat, PdelayQuantization)>, &'static str> {
    let Some((old_t3, old_t4)) = previous else {
        return Ok(None);
    };
    let numerator = t3.sub(old_t3)?;
    let interval = t4.sub(old_t4)?;
    if !numerator.positive() || !interval.positive() {
        return Err("nonpositive Pdelay sample interval");
    }
    let round_trip = t4.sub(t1)?;
    if round_trip.num < 0 {
        return Err("negative sampled Pdelay round trip");
    }
    let wire_quantum = Rat::new(125, 8192)?;
    let rate = numerator.div(interval)?;
    let rate_error = wire_quantum.div(interval)?;
    let rate_lower_open = rate.sub(rate_error)?;
    let rate_upper_open = rate.add(rate_error)?;
    // Deliberately require strict margin. At equality the open interval still
    // contains only positive rates, but this selected contract is conservative.
    if !rate_lower_open.positive() {
        return Err("sampled Pdelay rate lacks strictly positive quantization margin");
    }
    let alpha = round_trip.div(interval)?;
    let scale = if alpha.sub(Rat::ONE)?.positive() {
        alpha
    } else {
        Rat::ONE
    };
    let delay_error_bound = wire_quantum.mul(scale)?.half()?;
    let point_delay = rate.mul(round_trip)?.sub(t3.sub(t2)?)?.half()?;
    let delay_lower_open = point_delay.sub(delay_error_bound)?;
    let delay_upper_open = point_delay.add(delay_error_bound)?;
    let quantization_limited_zero = point_delay.num < 0;
    if quantization_limited_zero && !delay_upper_open.positive() {
        return Err("negative Pdelay outside open wire-quantization enclosure");
    }
    let estimate = if quantization_limited_zero {
        Rat::ZERO
    } else {
        point_delay
    };
    Ok(Some((
        rate,
        estimate,
        PdelayQuantization {
            delay_units: PdelayDelayUnits::NeighborRawClockPicoseconds,
            point_delay,
            wire_quantum,
            delay_error_bound,
            delay_lower_open,
            delay_upper_open,
            rate_lower_open,
            rate_upper_open,
            quantization_limited_zero,
        },
    )))
}

pub fn gm_ratio(
    cumulative_scaled_rate_offset: i32,
    neighbor_rate: Rat,
) -> Result<Rat, &'static str> {
    let upstream = Rat::new(
        (1i128 << 41) + i128::from(cumulative_scaled_rate_offset),
        1i128 << 41,
    )?;
    let ratio = upstream.mul(neighbor_rate)?;
    if !ratio.positive() {
        return Err("nonpositive GM rate ratio");
    }
    Ok(ratio)
}

pub fn sync_target(
    origin: Rat,
    correction: Rat,
    delay_neighbor: Rat,
    neighbor_rate: Rat,
    gm_local_ratio: Rat,
    sync_rx: Rat,
    now_raw: Rat,
) -> Result<(Rat, Rat), &'static str> {
    let upstream = sync_rx.sub(delay_neighbor.div(neighbor_rate)?)?;
    let at_ingress = origin
        .add(correction)?
        .add(delay_neighbor.mul(gm_local_ratio.div(neighbor_rate)?)?)?;
    let target = at_ingress.add(gm_local_ratio.mul(now_raw.sub(sync_rx)?)?)?;
    Ok((upstream, target))
}

pub fn relay_correction(
    correction: Rat,
    gm_local_ratio: Rat,
    egress_raw: Rat,
    upstream_tx_raw: Rat,
) -> Result<Rat, &'static str> {
    correction.add(gm_local_ratio.mul(egress_raw.sub(upstream_tx_raw)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn n(v: i128) -> Rat {
        Rat::new(v, 1).unwrap()
    }
    #[test]
    fn independent_peer_delay_rate_and_queue_turnaround() {
        // A raw=g, B raw=1.0001*g+500ns. Wire delay=100ns,
        // response turnaround=300ns; all numbers below are nanoseconds.
        let t1 = n(0);
        let t2 = Rat::new(60001, 100).unwrap();
        let t3 = Rat::new(90004, 100).unwrap();
        let t4 = n(500);
        let measured = pdelay(
            Some((t3.sub(n(1_000_100)).unwrap(), t4.sub(n(1_000_000)).unwrap())),
            t1,
            t2,
            t3,
            t4,
        )
        .unwrap()
        .unwrap();
        assert_eq!(measured.0, Rat::new(10001, 10000).unwrap());
        assert_eq!(measured.1, Rat::new(10001, 100).unwrap());
        assert_eq!(measured.1.div(measured.0).unwrap(), n(100));
        // A much longer responder queue residence changes t3/t4 together.
        let long_t3 = t3.add(n(2000)).unwrap();
        let long_t4 = t4
            .add(n(2000).div(Rat::new(10001, 10000).unwrap()).unwrap())
            .unwrap();
        let longer_queue = pdelay(
            Some((t3.sub(n(1_000_100)).unwrap(), t4.sub(n(1_000_000)).unwrap())),
            t1,
            t2,
            long_t3,
            long_t4,
        )
        .unwrap()
        .unwrap();
        assert_eq!(longer_queue, measured);
    }
    #[test]
    fn independent_relay_residence_and_downstream_sync() {
        let (u, target) = sync_target(
            n(1000),
            Rat::ZERO,
            n(100),
            Rat::ONE,
            Rat::ONE,
            n(1100),
            n(1100),
        )
        .unwrap();
        assert_eq!(u, n(1000));
        assert_eq!(target, n(1100));
        let c = relay_correction(Rat::ZERO, Rat::ONE, n(1500), u).unwrap();
        assert_eq!(c, n(500));
        let (_, downstream) =
            sync_target(n(1000), c, n(200), Rat::ONE, Rat::ONE, n(1700), n(1700)).unwrap();
        assert_eq!(downstream, n(1700));
    }
    #[test]
    fn fractional_neighbor_and_grandmaster_ratios_preserve_relay_residence_once() {
        let r = Rat::new(3, 2).unwrap();
        let q = Rat::new(5, 4).unwrap();
        let c = Rat::new(1, 2).unwrap();
        let (upstream, target) = sync_target(n(1000), c, n(150), r, q, n(2200), n(2300)).unwrap();
        assert_eq!(upstream, n(2100));
        assert_eq!(target, Rat::new(2501, 2).unwrap());
        let downstream_c = relay_correction(c, q, n(2600), upstream).unwrap();
        assert_eq!(downstream_c, Rat::new(1251, 2).unwrap());
        let (_, downstream) = sync_target(
            n(1000),
            downstream_c,
            n(80),
            Rat::new(4, 5).unwrap(),
            Rat::new(6, 5).unwrap(),
            n(3000),
            n(3040),
        )
        .unwrap();
        assert_eq!(downstream, Rat::new(3587, 2).unwrap());
    }
    #[test]
    fn opposite_oscillator_drift_and_large_checkpoint_rational() {
        // B runs 100 ppm slow. Its raw timestamp is 500 + 0.9999 * global ns;
        // the response spends 300 ns in B's local clock and the link is 100 ns.
        let r = Rat::new(9999, 10000).unwrap();
        let t1 = n(0);
        let t2 = Rat::new(59999, 100).unwrap();
        let t3 = Rat::new(89999, 100).unwrap();
        // The exact turnaround on A is 300/r; use an independent fraction to
        // avoid a rounded physical sample in the rate/delay assertion.
        let t4 = n(200).add(n(300).div(r).unwrap()).unwrap();
        let old_t3 = t3.sub(n(999900)).unwrap();
        let old_t4 = t4.sub(n(1000000)).unwrap();
        let (rate, delay) = pdelay(Some((old_t3, old_t4)), t1, t2, t3, t4)
            .unwrap()
            .unwrap();
        assert_eq!(rate, r);
        assert_eq!(delay, Rat::new(9999, 100).unwrap());
        let big = Rat::new(5219441373549569216168762263625, 7082414817198370455552).unwrap();
        let json = serde_json::to_string(&big).unwrap();
        assert!(json.contains("\"5219441373549569216168762263625\""));
        assert_eq!(serde_json::from_str::<Rat>(&json).unwrap(), big);
        assert!(serde_json::from_str::<Rat>("{\"num\":\"01\",\"den\":\"1\"}").is_err());
    }
    #[test]
    fn exact_delay_threshold_and_zero_delay_boundary() {
        let threshold = 100u64;
        assert!(
            Rat::new(199, 2)
                .unwrap()
                .within_integer_threshold(threshold)
        );
        assert!(n(100).within_integer_threshold(threshold));
        assert!(
            !Rat::new(201, 2)
                .unwrap()
                .within_integer_threshold(threshold)
        );
        assert!(!Rat::new(-1, 2).unwrap().within_integer_threshold(threshold));
        let (rate, delay) = pdelay(Some((n(0), n(0))), n(0), n(0), n(100), n(100))
            .unwrap()
            .unwrap();
        assert_eq!(rate, Rat::ONE);
        assert_eq!(delay, Rat::ZERO);
        assert!(delay.within_integer_threshold(0));
    }
    #[test]
    fn sampled_wire_enclosure_has_open_endpoints_and_preserves_positive_estimates() {
        for (num, den) in [(0, 1), (1, 68), (1, 1), (2, 1)] {
            let alpha = Rat::new(num, den).unwrap();
            let q = Rat::new(125, 8192).unwrap();
            let scale = if num > den { alpha } else { Rat::ONE };
            let bound = q.mul(scale).unwrap().half().unwrap();
            let t4 = n(268);
            let t3 = t4;
            let t1 = t4.sub(alpha.mul(n(68)).unwrap()).unwrap();
            let values = |point: Rat| {
                let t2 = t1.add(point.mul(n(2)).unwrap()).unwrap();
                pdelay_wire_quantized(Some((n(200), n(200))), t1, t2, t3, t4)
            };
            let positive = Rat::new(1, 3).unwrap();
            let (rate, delay, proof) = values(positive).unwrap().unwrap();
            assert_eq!((rate, delay), (Rat::ONE, positive));
            assert_eq!(proof.delay_error_bound, bound);
            assert_eq!(proof.delay_lower_open, positive.sub(bound).unwrap());
            assert_eq!(proof.delay_upper_open, positive.add(bound).unwrap());
            assert_eq!(
                proof.rate_lower_open,
                Rat::ONE.sub(q.div(n(68)).unwrap()).unwrap()
            );
            assert_eq!(
                proof.rate_upper_open,
                Rat::ONE.add(q.div(n(68)).unwrap()).unwrap()
            );
            assert!(!proof.quantization_limited_zero);
            assert_eq!(
                proof.delay_units,
                PdelayDelayUnits::NeighborRawClockPicoseconds
            );
            let inside = Rat::ZERO.sub(bound.half().unwrap()).unwrap();
            let (rate, estimate, proof) = values(inside).unwrap().unwrap();
            assert_eq!((rate, estimate), (Rat::ONE, Rat::ZERO));
            assert_eq!(proof.point_delay, inside);
            assert!(proof.quantization_limited_zero);
            assert!(
                values(Rat::ZERO.sub(bound).unwrap()).is_err(),
                "excluded upper endpoint0"
            );
            assert!(values(Rat::ZERO.sub(bound.mul(n(2)).unwrap()).unwrap()).is_err());
            let t2 = t1.add(inside.mul(n(2)).unwrap()).unwrap();
            assert!(
                pdelay(Some((n(200), n(200))), t1, t2, t3, t4).is_err(),
                "ideal negative rejection unchanged"
            );
        }
    }
    #[test]
    fn sampled_wire_enclosure_requires_explicit_positive_rate_margin_and_valid_intervals() {
        let q = Rat::new(125, 8192).unwrap();
        // Lower endpoint0 is excluded, hence all enclosed rates are positive;
        // the selected stricter-margin policy intentionally rejects equality.
        assert!(
            pdelay_wire_quantized(Some((n(0), n(0))), n(1), q.add(n(1)).unwrap(), q, n(1)).is_err()
        );
        let above = q.add(Rat::new(1, 1_000_000).unwrap()).unwrap();
        assert!(
            pdelay_wire_quantized(
                Some((n(0), n(0))),
                n(1),
                above.add(n(1)).unwrap(),
                above,
                n(1)
            )
            .unwrap()
            .is_some()
        );
        assert!(pdelay_wire_quantized(Some((n(0), n(1))), n(1), n(2), n(2), n(1)).is_err());
        assert!(pdelay_wire_quantized(Some((n(0), n(0))), n(2), n(2), n(2), n(1)).is_err());
        assert!(pdelay_wire_quantized(Some((n(2), n(0))), n(0), n(0), n(1), n(1)).is_err());
        assert_eq!(
            pdelay_wire_quantized(None, n(0), n(0), n(1), n(1)).unwrap(),
            None
        );
    }
}
