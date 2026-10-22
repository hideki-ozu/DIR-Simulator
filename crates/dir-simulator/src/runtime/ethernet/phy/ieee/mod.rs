//! Building blocks for named IEEE 802.3 physical modes.
//!
//! These functions do not themselves establish a named PHY: a mode requires
//! PCS, PMA, startup, medium and network attachment in the live execution path.
pub mod an_hsm;
pub mod mau;
pub mod plca;
pub mod repeater;
pub mod rs9;
pub mod t1_an;
pub mod t1_gigabit;
pub mod t1_info;
pub mod t1_link;
pub mod t1_raw;
pub mod t1_stream;
pub mod t1_training;
pub mod t1s;
