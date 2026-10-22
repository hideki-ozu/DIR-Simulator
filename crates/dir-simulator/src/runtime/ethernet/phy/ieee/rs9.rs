//! Clause 97.3.2.2.11 shortened Reed–Solomon (450,406) code.
//!
//! Input and output arrays are in wire symbol order: D405..D0, then P43..P0.
//! Each element has 9 bits, whose least significant bit is sent first. Field
//! reduction is x^9 + x^4 + 1; generator roots are alpha^0..alpha^43.
use super::super::Result;

pub const DATA_SYMBOLS: usize = 406;
pub const PARITY_SYMBOLS: usize = 44;
pub const CODE_SYMBOLS: usize = DATA_SYMBOLS + PARITY_SYMBOLS;
pub const CORRECTION_RADIUS: usize = PARITY_SYMBOLS / 2;
const FIELD_ORDER: usize = 511;
const FIELD_MASK: u16 = 0x1ff;

/// Multiplication in GF(2^9), using the Clause 97 primitive polynomial.
pub fn multiply(mut a: u16, mut b: u16) -> u16 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        b >>= 1;
        a <<= 1;
        if a & 0x200 != 0 {
            a ^= 0x211;
        }
    }
    product & FIELD_MASK
}

fn power(mut base: u16, mut exponent: usize) -> u16 {
    let mut value = 1;
    while exponent != 0 {
        if exponent & 1 != 0 {
            value = multiply(value, base);
        }
        base = multiply(base, base);
        exponent >>= 1;
    }
    value
}

fn alpha(exponent: usize) -> u16 {
    power(2, exponent % FIELD_ORDER)
}

fn inverse(value: u16) -> Result<u16> {
    if value == 0 {
        return Err("RS division by zero".into());
    }
    Ok(power(value, FIELD_ORDER - 1))
}

fn validate(symbols: &[u16], expected: usize) -> Result<()> {
    if symbols.len() != expected || symbols.iter().any(|&s| s > FIELD_MASK) {
        return Err(format!("RS expects {expected} nine-bit symbols"));
    }
    Ok(())
}

/// Monic coefficients in descending powers, produced without wire data.
fn generator() -> [u16; PARITY_SYMBOLS + 1] {
    let mut coefficients = [0u16; PARITY_SYMBOLS + 1];
    coefficients[0] = 1;
    for degree in 0..PARITY_SYMBOLS {
        let root = alpha(degree);
        for index in (1..=degree + 1).rev() {
            coefficients[index] ^= multiply(root, coefficients[index - 1]);
        }
    }
    coefficients
}

/// Systematic encoder. Polynomial long division over the 450-symbol vector is
/// equivalent to the zero-initialized Clause 97 parity shift register.
pub fn encode(data: &[u16]) -> Result<[u16; CODE_SYMBOLS]> {
    validate(data, DATA_SYMBOLS)?;
    let generator = generator();
    let mut output = [0u16; CODE_SYMBOLS];
    output[..DATA_SYMBOLS].copy_from_slice(data);
    for position in 0..DATA_SYMBOLS {
        let feedback = output[position];
        if feedback != 0 {
            for offset in 1..=PARITY_SYMBOLS {
                output[position + offset] ^= multiply(feedback, generator[offset]);
            }
        }
    }
    output[..DATA_SYMBOLS].copy_from_slice(data);
    Ok(output)
}

/// Syndrome order follows the 44 generator roots, alpha^0 first.
pub fn syndromes(code: &[u16]) -> Result<[u16; PARITY_SYMBOLS]> {
    validate(code, CODE_SYMBOLS)?;
    let mut result = [0u16; PARITY_SYMBOLS];
    for (root_index, syndrome) in result.iter_mut().enumerate() {
        let root = alpha(root_index);
        for &symbol in code {
            *syndrome = multiply(*syndrome, root) ^ symbol;
        }
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub data: [u16; DATA_SYMBOLS],
    pub corrected_symbols: usize,
}

/// Corrects at most 22 unknown nine-bit symbol errors. A pattern above that
/// radius can fail or miscorrect, as for any bounded-distance RS decoder.
pub fn decode(received: &[u16]) -> Result<Decoded> {
    validate(received, CODE_SYMBOLS)?;
    let synd = syndromes(received)?;
    if synd.iter().all(|&s| s == 0) {
        let mut data = [0; DATA_SYMBOLS];
        data.copy_from_slice(&received[..DATA_SYMBOLS]);
        return Ok(Decoded {
            data,
            corrected_symbols: 0,
        });
    }

    // Berlekamp–Massey determines Lambda(z)=product(1-X_i*z), where X_i
    // is alpha^(449-wire_index). This is distinct from encoder division.
    let mut locator = [0u16; PARITY_SYMBOLS + 1];
    let mut previous = [0u16; PARITY_SYMBOLS + 1];
    locator[0] = 1;
    previous[0] = 1;
    let mut degree = 0usize;
    let mut shift = 1usize;
    let mut previous_discrepancy = 1u16;
    for n in 0..PARITY_SYMBOLS {
        let mut discrepancy = synd[n];
        for i in 1..=degree {
            discrepancy ^= multiply(locator[i], synd[n - i]);
        }
        if discrepancy == 0 {
            shift += 1;
            continue;
        }
        let factor = multiply(discrepancy, inverse(previous_discrepancy)?);
        let old = locator;
        for i in 0..=(PARITY_SYMBOLS - shift) {
            locator[i + shift] ^= multiply(factor, previous[i]);
        }
        if 2 * degree <= n {
            degree = n + 1 - degree;
            previous = old;
            previous_discrepancy = discrepancy;
            shift = 1;
        } else {
            shift += 1;
        }
    }
    if degree == 0 || degree > CORRECTION_RADIUS {
        return Err("uncorrectable RS codeword: locator degree".into());
    }
    let mut locations = Vec::with_capacity(degree);
    for wire_index in 0..CODE_SYMBOLS {
        let exponent = CODE_SYMBOLS - 1 - wire_index;
        let inverse_x = alpha(FIELD_ORDER - exponent % FIELD_ORDER);
        let mut value = 0;
        for &coefficient in locator[..=degree].iter().rev() {
            value = multiply(value, inverse_x) ^ coefficient;
        }
        if value == 0 {
            locations.push(wire_index);
        }
    }
    if locations.len() != degree {
        return Err("uncorrectable RS codeword: locator roots".into());
    }

    // Solve the syndrome Vandermonde system for error magnitudes. This avoids
    // relying on an encoder-derived parity estimate or a Forney orientation.
    let mut matrix = vec![vec![0u16; degree + 1]; degree];
    for row in 0..degree {
        matrix[row][degree] = synd[row];
        for (column, &wire_index) in locations.iter().enumerate() {
            matrix[row][column] = alpha(((CODE_SYMBOLS - 1 - wire_index) * row) % FIELD_ORDER);
        }
    }
    for pivot in 0..degree {
        let Some(nonzero) = (pivot..degree).find(|&row| matrix[row][pivot] != 0) else {
            return Err("uncorrectable RS codeword: singular magnitudes".into());
        };
        matrix.swap(pivot, nonzero);
        let scale = inverse(matrix[pivot][pivot])?;
        for col in pivot..=degree {
            matrix[pivot][col] = multiply(matrix[pivot][col], scale);
        }
        for row in 0..degree {
            if row == pivot {
                continue;
            }
            let factor = matrix[row][pivot];
            for col in pivot..=degree {
                matrix[row][col] ^= multiply(factor, matrix[pivot][col]);
            }
        }
    }
    let mut corrected = received.to_vec();
    for (column, &wire_index) in locations.iter().enumerate() {
        corrected[wire_index] ^= matrix[column][degree];
    }
    if syndromes(&corrected)?.iter().any(|&s| s != 0) {
        return Err("uncorrectable RS codeword: residual syndrome".into());
    }
    let mut data = [0u16; DATA_SYMBOLS];
    data.copy_from_slice(&corrected[..DATA_SYMBOLS]);
    Ok(Decoded {
        data,
        corrected_symbols: degree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_primitive_and_zero() {
        assert_eq!(alpha(0), 1);
        assert_eq!(alpha(9), 0x11);
        assert_eq!(alpha(FIELD_ORDER), 1);
        assert_eq!(multiply(0, 0x1ff), 0);
        assert_ne!(alpha(1), 1);
        for i in 1..FIELD_ORDER {
            assert_ne!(alpha(i), 1);
        }
    }
    #[test]
    fn independent_literal_vectors_and_correction_limits() {
        let mut data = [0u16; DATA_SYMBOLS];
        let zero = encode(&data).unwrap();
        assert!(zero.iter().all(|&s| s == 0));
        data[0] = 1;
        let one = encode(&data).unwrap();
        assert_eq!(&one[..3], &[1, 0, 0]);
        // Generated by tests/fixtures/generate_t1_rs9_reference.py, which
        // solves 44 root equations rather than dividing by G(z). JSON
        // {"parity":[...]} SHA-256: ee900525c4d251516aa3c7813fc96fe40ddfbba31047cbfd3812b8d41a486723.
        assert_eq!(
            &one[DATA_SYMBOLS..],
            &[
                356, 6, 376, 343, 157, 498, 180, 322, 272, 190, 63, 175, 349, 419, 431, 388, 244,
                229, 300, 499, 125, 95, 393, 271, 115, 274, 243, 53, 282, 72, 159, 139, 436, 318,
                472, 151, 174, 364, 427, 113, 331, 337, 238, 412,
            ]
        );
        assert!(syndromes(&one).unwrap().iter().all(|&s| s == 0));
        for count in [1, 21, 22] {
            let mut damaged = one;
            for i in 0..count {
                damaged[(i * 19) % CODE_SYMBOLS] ^= ((i * 37 + 1) & 0x1ff) as u16;
            }
            let decoded = decode(&damaged).unwrap();
            assert_eq!(decoded.data, data);
            assert_eq!(decoded.corrected_symbols, count);
        }
        let mut damaged = one;
        for (i, symbol) in damaged.iter_mut().enumerate().take(23) {
            *symbol ^= (i + 1) as u16;
        }
        assert!(decode(&damaged).is_err());
    }
}
