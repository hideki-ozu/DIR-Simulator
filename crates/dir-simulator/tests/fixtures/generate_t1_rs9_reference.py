"""Independent Clause 97 RS(450,406) reference vector.

The production encoder divides by a generator polynomial. This script instead
solves the 44 root-evaluation equations for the parity symbols by Gaussian
elimination. It prints parity for D405=1 and all other data symbols zero.
Requires only Python 3; no production Rust is imported.
"""
import hashlib
import json

FIELD_ORDER = 511
PARITY = 44
LENGTH = 450


def mul(a, b):
    result = 0
    while b:
        if b & 1:
            result ^= a
        b >>= 1
        a <<= 1
        if a & 0x200:
            a ^= 0x211
    return result


def exp(base, n):
    value = 1
    for _ in range(n):
        value = mul(value, base)
    return value


def alpha(n):
    return exp(2, n % FIELD_ORDER)


rows = []
for root in range(PARITY):
    rows.append(
        [alpha((PARITY - 1 - column) * root) for column in range(PARITY)]
        + [alpha((LENGTH - 1) * root)]
    )
for column in range(PARITY):
    pivot = next(row for row in range(column, PARITY) if rows[row][column])
    rows[column], rows[pivot] = rows[pivot], rows[column]
    inverse = exp(rows[column][column], FIELD_ORDER - 1)
    rows[column] = [mul(v, inverse) for v in rows[column]]
    for row in range(PARITY):
        if row != column:
            factor = rows[row][column]
            rows[row] = [
                value ^ mul(factor, rows[column][index])
                for index, value in enumerate(rows[row])
            ]
parity = [row[-1] for row in rows]
wire = [1] + [0] * 405 + parity
for root in range(PARITY):
    x = alpha(root)
    syndrome = 0
    for symbol in wire:
        syndrome = mul(syndrome, x) ^ symbol
    assert syndrome == 0
blob = json.dumps({"parity": parity}, separators=(",", ":")).encode()
print(blob.decode())
print("sha256", hashlib.sha256(blob).hexdigest())
