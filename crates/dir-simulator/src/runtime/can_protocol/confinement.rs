//! Bosch CAN 2.0 B section 7 counter rules, kept independently testable.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub tec: u16,
    pub rec: u16,
}
impl Counters {
    pub fn state(&self) -> &'static str {
        if self.tec >= 256 {
            "bus_off"
        } else if self.tec >= 128 || self.rec >= 128 {
            "error_passive"
        } else {
            "error_active"
        }
    }
    pub fn tx_error(&mut self, ack_no_dominant: bool, arbitration_stuff_exception: bool) {
        if !(arbitration_stuff_exception || ack_no_dominant && self.state() == "error_passive") {
            self.tec = self.tec.saturating_add(8).min(256);
        }
    }
    pub fn rx_error(&mut self, bit_during_active_flag: bool) {
        if !bit_during_active_flag {
            self.rec = self.rec.saturating_add(1).min(255);
        }
    }
    pub fn dominant_after_flag(&mut self) {
        self.rec = self.rec.saturating_add(8).min(255);
    }
    pub fn flag_bit_error(&mut self, transmitter: bool) {
        if transmitter {
            self.tec = self.tec.saturating_add(8).min(256);
        } else {
            self.rec = self.rec.saturating_add(8).min(255);
        }
    }
    pub fn dominant_extension(&mut self, transmitter: bool) {
        self.flag_bit_error(transmitter);
    }
    pub fn tx_success(&mut self) {
        self.tec = self.tec.saturating_sub(1);
    }
    pub fn rx_success(&mut self) {
        if self.rec > 127 {
            self.rec = 120;
        } else {
            self.rec = self.rec.saturating_sub(1);
        }
    }
}
