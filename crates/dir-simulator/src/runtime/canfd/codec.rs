//! Content-derived ISO CAN FD data frames (ISO 11898-1:2015 wire format).
//!
//! Logical dominant/recessive bits, from SOF through EOF; intermission is separate.
//! This ideal, successfully acknowledged frame codec does not model a transceiver,
//! synchronization, error confinement, retransmission, or physical-layer faults.
//! CRC includes dynamic stuff bits and the Gray-coded stuff count with parity,
//! excludes fixed stuff bits, and starts with only its most significant bit set.

use std::fmt;

pub const DATA_LENGTHS: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: u32,
    pub extended: bool,
    pub brs: bool,
    /// Recessive means the transmitter is error passive.
    pub esi: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);
impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CodecError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatePhase {
    Nominal,
    Data,
    /// Nominal up to the nominal sample point; data after the data sample point.
    Brs,
    /// Data up to the data sample point; nominal after the nominal sample point.
    CrcDelimiter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireFrame {
    pub frame: Vec<u8>,
    pub arbitration: Vec<u8>,
    pub crc_input: Vec<u8>,
    pub crc_width: u8,
    pub crc: u32,
    pub dlc: u8,
    pub stuff_count: u8,
    /// The three Gray bits followed by even parity, MSB first.
    pub stuff_count_code: u8,
    pub dynamic_stuff_positions: Vec<usize>,
    pub fixed_stuff_positions: Vec<usize>,
    pub brs_index: usize,
    pub crc_delimiter_index: usize,
    pub brs: bool,
}

fn bits(out: &mut Vec<u8>, value: u32, width: u8) {
    out.extend((0..width).rev().map(|i| ((value >> i) & 1) as u8));
}

pub fn encode(input: &Frame) -> Result<WireFrame, CodecError> {
    if input.id > if input.extended { 0x1fff_ffff } else { 0x7ff } {
        return Err(CodecError("CAN FD identifier exceeds format width".into()));
    }
    let dlc = DATA_LENGTHS
        .iter()
        .position(|&n| n == input.data.len())
        .ok_or_else(|| {
            CodecError("CAN FD data length must be 0..8, 12, 16, 20, 24, 32, 48 or 64 bytes".into())
        })? as u8;
    let mut arbitration = Vec::new();
    if input.extended {
        bits(&mut arbitration, input.id >> 18, 11);
        arbitration.extend([1, 1]); // SRR, IDE
        bits(&mut arbitration, input.id & 0x3ffff, 18);
        arbitration.push(0); // RRS
    } else {
        bits(&mut arbitration, input.id, 11);
        arbitration.extend([0, 0]); // RRS, IDE
    }
    let mut raw = vec![0]; // SOF
    raw.extend(&arbitration);
    raw.extend([1, 0]); // FDF, res
    let raw_brs = raw.len();
    raw.extend([u8::from(input.brs), u8::from(input.esi)]);
    bits(&mut raw, dlc as u32, 4);
    for &byte in &input.data {
        bits(&mut raw, byte as u32, 8);
    }

    let mut frame = Vec::new();
    let mut dynamic_stuff_positions = Vec::new();
    let (mut last, mut run) = (2, 0);
    let mut brs_index = 0;
    for (index, &bit) in raw.iter().enumerate() {
        if index == raw_brs {
            brs_index = frame.len();
        }
        frame.push(bit);
        run = if bit == last { run + 1 } else { 1 };
        last = bit;
        // The CRC field starts with its own fixed stuff bit, even at a run of five.
        if run == 5 && index + 1 < raw.len() {
            dynamic_stuff_positions.push(frame.len());
            last ^= 1;
            frame.push(last);
            run = 1;
        }
    }
    let stuff_count = (dynamic_stuff_positions.len() % 8) as u8;
    let gray = stuff_count ^ (stuff_count >> 1);
    let stuff_count_code = (gray << 1) | (gray.count_ones() as u8 & 1);
    let mut crc_input = frame.clone();
    bits(&mut crc_input, stuff_count_code as u32, 4);
    let (crc_width, polynomial) = if input.data.len() <= 16 {
        (17, 0x1685b)
    } else {
        (21, 0x102899)
    };
    let mut crc = 1 << (crc_width - 1);
    for &bit in &crc_input {
        let feedback = (crc >> (crc_width - 1)) ^ bit as u32;
        crc = (crc << 1) & ((1 << crc_width) - 1);
        if feedback != 0 {
            crc ^= polynomial;
        }
    }
    let mut crc_field = Vec::new();
    bits(&mut crc_field, stuff_count_code as u32, 4);
    bits(&mut crc_field, crc, crc_width);
    let mut fixed_stuff_positions = Vec::new();
    for (index, bit) in crc_field.into_iter().enumerate() {
        if index % 4 == 0 {
            fixed_stuff_positions.push(frame.len());
            frame.push(frame.last().copied().unwrap() ^ 1);
        }
        frame.push(bit);
    }
    let crc_delimiter_index = frame.len();
    frame.extend([1, 0, 1]); // CRC delimiter, acknowledged ACK slot, ACK delimiter
    frame.extend([1; 7]); // EOF
    Ok(WireFrame {
        frame,
        arbitration,
        crc_input,
        crc_width,
        crc,
        dlc,
        stuff_count,
        stuff_count_code,
        dynamic_stuff_positions,
        fixed_stuff_positions,
        brs_index,
        crc_delimiter_index,
        brs: input.brs,
    })
}

impl WireFrame {
    pub fn rate_phase(&self, index: usize) -> Option<RatePhase> {
        if index >= self.frame.len() {
            return None;
        }
        Some(
            if !self.brs || index < self.brs_index || index > self.crc_delimiter_index {
                RatePhase::Nominal
            } else if index == self.brs_index {
                RatePhase::Brs
            } else if index == self.crc_delimiter_index {
                RatePhase::CrcDelimiter
            } else {
                RatePhase::Data
            },
        )
    }

    /// Effective whole-frame phase counts. The two split bits together occupy
    /// one nominal and one data bit, independent of the chosen sample points.
    pub fn phase_bits(&self) -> (u64, u64) {
        if self.brs {
            let data = (self.crc_delimiter_index - self.brs_index) as u64;
            (self.frame.len() as u64 - data, data)
        } else {
            (self.frame.len() as u64, 0)
        }
    }

    /// Ceiling of the complete rational frame duration, rounded only once.
    pub fn duration_ps(&self, nominal_bps: u64, data_bps: u64) -> Result<u64, CodecError> {
        if nominal_bps == 0 || data_bps == 0 {
            return Err(CodecError("CAN FD bitrates must be nonzero".into()));
        }
        let (nominal, data) = self.phase_bits();
        let numerator = (nominal as u128 * data_bps as u128 + data as u128 * nominal_bps as u128)
            .checked_mul(1_000_000_000_000)
            .ok_or_else(|| CodecError("CAN FD duration overflow".into()))?;
        let duration = numerator.div_ceil(nominal_bps as u128 * data_bps as u128);
        u64::try_from(duration).map_err(|_| CodecError("CAN FD duration overflow".into()))
    }
}

/// Decode one complete, ideally acknowledged ISO frame without expected content.
/// The input ends at EOF: intermission, missing ACK and error flags are rejected.
pub fn decode(captured: &[u8]) -> Result<Frame, CodecError> {
    struct Reader<'a> {
        bits: &'a [u8],
        index: usize,
        last: u8,
        run: u8,
        stuff: usize,
    }
    impl Reader<'_> {
        fn bit(&mut self) -> Result<u8, CodecError> {
            let value = self
                .bits
                .get(self.index)
                .copied()
                .ok_or_else(|| CodecError("truncated CAN FD frame".into()))?;
            self.index += 1;
            if value > 1 {
                return Err(CodecError("CAN FD wire contains a non-bit".into()));
            }
            Ok(value)
        }
        fn dynamic(&mut self) -> Result<u8, CodecError> {
            if self.run == 5 {
                let value = self.bit()?;
                if value == self.last {
                    return Err(CodecError("CAN FD dynamic stuff error".into()));
                }
                self.last = value;
                self.run = 1;
                self.stuff += 1;
            }
            let value = self.bit()?;
            self.run = if value == self.last { self.run + 1 } else { 1 };
            self.last = value;
            Ok(value)
        }
        fn number(&mut self, width: usize) -> Result<u32, CodecError> {
            let mut value = 0;
            for _ in 0..width {
                value = (value << 1) | self.dynamic()? as u32;
            }
            Ok(value)
        }
        fn expect(&mut self, value: u8) -> Result<(), CodecError> {
            if self.dynamic()? != value {
                return Err(CodecError("CAN FD reserved/form error".into()));
            }
            Ok(())
        }
    }
    let mut r = Reader {
        bits: captured,
        index: 0,
        last: 2,
        run: 0,
        stuff: 0,
    };
    r.expect(0)?; // SOF
    let base = r.number(11)?;
    let rrs_srr = r.dynamic()?;
    let extended = r.dynamic()? != 0;
    let id = if extended {
        if rrs_srr != 1 {
            return Err(CodecError("CAN FD extended SRR must be recessive".into()));
        }
        let id = (base << 18) | r.number(18)?;
        r.expect(0)?; // RRS
        id
    } else {
        if rrs_srr != 0 {
            return Err(CodecError("CAN FD base RRS must be dominant".into()));
        }
        base
    };
    r.expect(1)?; // FDF
    r.expect(0)?; // res
    let brs = r.dynamic()? != 0;
    let esi = r.dynamic()? != 0;
    let dlc = r.number(4)? as usize;
    let mut data = Vec::with_capacity(DATA_LENGTHS[dlc]);
    for _ in 0..DATA_LENGTHS[dlc] {
        data.push(r.number(8)? as u8);
    }
    let dynamic_end = r.index;
    let crc_width = if data.len() <= 16 { 17 } else { 21 };
    let mut crc_field = Vec::with_capacity(4 + crc_width);
    // Fixed stuffing starts immediately after the last payload/DLC bit, even
    // when the last dynamic run has reached five. Never consume a trailing
    // dynamic stuff bit here.
    for index in 0..4 + crc_width {
        if index % 4 == 0 {
            let previous = captured[r.index - 1];
            if r.bit()? == previous {
                return Err(CodecError("CAN FD fixed stuff error".into()));
            }
        }
        crc_field.push(r.bit()?);
    }
    let number = |bits: &[u8]| bits.iter().fold(0u32, |n, &bit| (n << 1) | bit as u32);
    let code = number(&crc_field[..4]) as u8;
    let count = (r.stuff % 8) as u8;
    let gray = count ^ (count >> 1);
    if code != (gray << 1) | (gray.count_ones() as u8 & 1) {
        return Err(CodecError("CAN FD stuff count/parity error".into()));
    }
    let polynomial = if crc_width == 17 { 0x1685b } else { 0x102899 };
    let mut crc = 1 << (crc_width - 1);
    for &bit in captured[..dynamic_end].iter().chain(&crc_field[..4]) {
        let feedback = (crc >> (crc_width - 1)) ^ bit as u32;
        crc = (crc << 1) & ((1 << crc_width) - 1);
        if feedback != 0 {
            crc ^= polynomial;
        }
    }
    if crc != number(&crc_field[4..]) {
        return Err(CodecError("CAN FD CRC mismatch".into()));
    }
    for expected in [1, 0, 1, 1, 1, 1, 1, 1, 1, 1] {
        if r.bit()? != expected {
            return Err(CodecError("CAN FD delimiter/ACK/EOF error".into()));
        }
    }
    if r.index != captured.len() {
        return Err(CodecError("trailing CAN FD wire bits".into()));
    }
    Ok(Frame {
        id,
        extended,
        brs,
        esi,
        data,
    })
}

/// Verify a captured ideal, acknowledged frame against its expected content.
/// Includes CRC, every stuff bit, reserved bits, ACK, delimiters and EOF.
pub fn verify(input: &Frame, captured: &[u8]) -> Result<(), CodecError> {
    let expected = encode(input)?;
    if captured.len() != expected.frame.len() {
        return Err(CodecError("CAN FD wire length mismatch".into()));
    }
    if let Some(index) = captured
        .iter()
        .zip(expected.frame)
        .position(|(&a, b)| a != b)
    {
        return Err(CodecError(format!("CAN FD wire mismatch at bit {index}")));
    }
    Ok(())
}
