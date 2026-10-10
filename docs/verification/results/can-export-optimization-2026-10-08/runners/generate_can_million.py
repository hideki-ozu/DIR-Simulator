#!/usr/bin/env python3
"""Generate deterministic million-request Classical CAN workload inputs.

The timing model is calculated locally from the standard-frame fields, CRC-15,
and dynamic bit stuffing. It does not call the DIR Simulator runtime.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


SENDER_COUNT = 32
BITRATE_BPS = 500_000
BIT_TIME_PS = 1_000_000_000_000 // BITRATE_BPS
FRAME_DATA = bytes(8)
CRC_POLYNOMIAL = 0x4599
POLYNOMIAL_WITH_TOP_BIT = (1 << 15) | CRC_POLYNOMIAL
LOADS = (("0.30", 3, 10), ("0.90", 9, 10), ("1.20", 6, 5))
U64_MAX = (1 << 64) - 1


def crc_register(bits: str) -> int:
    """Return the CAN CRC-15 remainder using the bit-serial register form."""
    register = 0
    for digit in bits:
        feedback = int(digit) ^ (register >> 14)
        register = (register << 1) & 0x7FFF
        if feedback:
            register ^= CRC_POLYNOMIAL
    return register


def crc_polynomial_division(bits: str) -> int:
    """Return the same remainder using independent integer polynomial division."""
    dividend = int(bits, 2) << 15
    while dividend.bit_length() >= POLYNOMIAL_WITH_TOP_BIT.bit_length():
        dividend ^= POLYNOMIAL_WITH_TOP_BIT << (
            dividend.bit_length() - POLYNOMIAL_WITH_TOP_BIT.bit_length()
        )
    return dividend


def frame_crc_input(identifier: int) -> str:
    """Build SOF through data fields for one 11-bit, eight-zero-byte data frame."""
    if not 0 <= identifier <= 0x7FF:
        raise ValueError(f"standard CAN identifier out of range: {identifier}")
    fields = (
        (0, 1),  # SOF
        (identifier, 11),
        (0, 1),  # RTR: data frame
        (0, 1),  # IDE: standard format
        (0, 1),  # r0
        (len(FRAME_DATA), 4),
        *((byte, 8) for byte in FRAME_DATA),
    )
    return "".join(f"{value:0{width}b}" for value, width in fields)


def stuff_bits(bits: str) -> tuple[str, int]:
    """Apply CAN dynamic stuffing through the CRC sequence, before CRC delimiter."""
    encoded: list[str] = []
    previous: str | None = None
    run_length = 0
    stuff_count = 0
    for digit in bits:
        encoded.append(digit)
        run_length = run_length + 1 if digit == previous else 1
        previous = digit
        if run_length == 5:
            previous = "1" if digit == "0" else "0"
            encoded.append(previous)
            run_length = 1
            stuff_count += 1
    return "".join(encoded), stuff_count


def sender_timing(index: int) -> dict[str, int | str]:
    identifier = 0x100 + index
    message_bits = frame_crc_input(identifier)
    register_crc = crc_register(message_bits)
    division_crc = crc_polynomial_division(message_bits)
    if register_crc != division_crc:
        raise AssertionError(f"CRC implementations disagree for sender {index}")
    raw_stuffable = message_bits + f"{register_crc:015b}"
    _, stuffing_count = stuff_bits(raw_stuffable)
    frame_bits = len(raw_stuffable) + stuffing_count + 10
    occupied_bits = frame_bits + 3
    return {
        "sender_index": index,
        "generator_id": f"n{index:02d}",
        "node": f"Main.n{index:02d}",
        "can_id": identifier,
        "crc15": register_crc,
        "stuff_bits": stuffing_count,
        "frame_bits_including_sof_to_eof": frame_bits,
        "intermission_bits": 3,
        "occupied_bits_including_intermission": occupied_bits,
        "C_i_ps": occupied_bits * BIT_TIME_PS,
    }


def make_ned() -> str:
    lines = [
        "package canmillion;",
        "simple Controller {",
        " parameters:",
        '  @class("dir.can.Controller");',
        "  int queueCapacity = default(64);",
        "  double txProcessingDelay @unit(s) = default(0ps);",
        "  double rxProcessingDelay @unit(s) = default(0ps);",
        '  string rxFilter = default("*");',
        " gates: output tx; input rx;",
        "}",
        "simple Bus {",
        " parameters:",
        '  @class("dir.can.Bus");',
        "  double bitrate @unit(bps);",
        '  string profile = default("can.cc.ideal.v1");',
        " gates:",
    ]
    for index in range(SENDER_COUNT):
        name = f"n{index:02d}"
        lines.extend((f"  input rx_{name};", f"  output tx_{name};"))
    lines.extend(
        (
            "}",
            "channel Wire {",
            " parameters:",
            '  @class("dir.link.FixedDelay");',
            "  double delay @unit(s) = default(0ps);",
            "}",
            "network Main {",
            " submodules:",
        )
    )
    for index in range(SENDER_COUNT):
        lines.append(f"  n{index:02d}: canmillion.Controller;")
    lines.extend(("  bus: canmillion.Bus;", " connections:"))
    for index in range(SENDER_COUNT):
        name = f"n{index:02d}"
        lines.append(f"  {name}.tx --> canmillion.Wire --> bus.rx_{name};")
        lines.append(f"  bus.tx_{name} --> canmillion.Wire --> {name}.rx;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def make_workload(senders: list[dict[str, int | str]], period_ps: int, count: int) -> str:
    generators = []
    for sender in senders:
        index = int(sender["sender_index"])
        generators.append(
            {
                "id": sender["generator_id"],
                "kind": "can.periodic.v1",
                "node": sender["node"],
                "start": "0ps",
                "phase": f"{(index * period_ps) // SENDER_COUNT}ps",
                "period": f"{period_ps}ps",
                "count": count,
                "frame": {
                    "format": "standard",
                    "id": sender["can_id"],
                    "data": FRAME_DATA.hex(),
                },
            }
        )
    return json.dumps(
        {"schema_version": 1, "generators": generators},
        indent=2,
        sort_keys=False,
    ) + "\n"


def make_ini(load_label: str, workload_name: str, total_time_ps: int) -> str:
    lines = [
        "[General]",
        "network = canmillion.Main",
        'ned-path = "models"',
        f"sim-time-limit = {total_time_ps}ps",
        "metrics-window = 1ms",
        "Main.bus.bitrate = 500kbps",
        f'workload = "{workload_name}"',
    ]
    for index in range(SENDER_COUNT):
        node = f"Main.n{index:02d}"
        lines.extend(
            (
                f"{node}.queueCapacity = 64",
                f'{node}.rxFilter = "*"',
                f"{node}.txProcessingDelay = 0ps",
                f"{node}.rxProcessingDelay = 0ps",
            )
        )
    lines.append(f"# Offered load rho = {load_label}")
    return "\n".join(lines) + "\n"


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def generate(output_dir: Path, count: int) -> dict[str, object]:
    if count <= 0 or count > U64_MAX:
        raise ValueError("count-per-sender must be in 1..=18446744073709551615")
    senders = [sender_timing(index) for index in range(SENDER_COUNT)]
    total_occupied_ps = sum(int(sender["C_i_ps"]) for sender in senders)
    input_files: dict[str, bytes] = {}
    ned_path = Path("models/canmillion/Main.ned")
    input_files[ned_path.as_posix()] = make_ned().encode("utf-8")
    scenarios: list[dict[str, object]] = []
    for label, rho_num, rho_den in LOADS:
        period_ps = (total_occupied_ps * rho_den + rho_num - 1) // rho_num
        total_time_ps = count * period_ps
        if period_ps > U64_MAX or total_time_ps > U64_MAX:
            raise ValueError(
                f"rho {label} period or simulation limit exceeds the simulator's u64 ps range"
            )
        phases = []
        for sender in senders:
            index = int(sender["sender_index"])
            phase = index * period_ps // SENDER_COUNT
            last_start = phase + (count - 1) * period_ps
            if phase >= period_ps or last_start >= total_time_ps:
                raise AssertionError(f"invalid periodic bounds for sender {index}")
            phases.append(
                {
                    "sender_index": index,
                    "node": sender["node"],
                    "phase_ps": phase,
                    "last_start_ps": last_start,
                }
            )
        workload_name = f"rho-{label}.workload.json"
        ini_name = f"rho-{label}.ini"
        workload_path = Path(workload_name)
        ini_path = Path(ini_name)
        input_files[workload_path.as_posix()] = make_workload(
            senders, period_ps, count
        ).encode("utf-8")
        input_files[ini_path.as_posix()] = make_ini(
            label, workload_name, total_time_ps
        ).encode("utf-8")
        scenarios.append(
            {
                "rho": label,
                "rho_exact": {"numerator": rho_num, "denominator": rho_den},
                "total_C_i_ps": total_occupied_ps,
                "P_ps": period_ps,
                "T_ps": total_time_ps,
                "request_count_per_sender": count,
                "request_count_total": SENDER_COUNT * count,
                "ini": ini_path.as_posix(),
                "workload": workload_path.as_posix(),
                "senders": phases,
            }
        )

    for relative_path, content in input_files.items():
        destination = output_dir / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)

    metadata: dict[str, object] = {
        "schema_version": 1,
        "generator": "scripts/performance/generate_can_million.py",
        "provenance": (
            "Deterministic analytic Classical CAN timing inputs; CRC-15 register and "
            "polynomial-division calculations agree. This does not claim simulator "
            "execution, performance, hardware capture, or ISO conformance."
        ),
        "count_per_sender": count,
        "sender_count": SENDER_COUNT,
        "request_count_per_configuration": SENDER_COUNT * count,
        "bitrate_bps": BITRATE_BPS,
        "bit_time_ps": BIT_TIME_PS,
        "frame": {
            "format": "standard",
            "can_id_first": 0x100,
            "can_id_last": 0x100 + SENDER_COUNT - 1,
            "data_hex": FRAME_DATA.hex(),
        },
        "calculation": {
            "crc_polynomial_hex": f"0x{CRC_POLYNOMIAL:04X}",
            "crc_input": "SOF through data field, before CRC sequence",
            "dynamic_stuffing": "SOF through CRC sequence inclusive",
            "unstuffed_trailing_bits": 10,
            "intermission_bits": 3,
            "C_i_definition": "(frame_bits_including_sof_to_eof + 3) * bit_time_ps",
            "rho_definition": "sum(C_i) / P",
            "period_rounding": "ceil(sum(C_i) / rho), using exact rational arithmetic",
            "phase_definition": "floor(sender_index * P / 32), with zero-based index",
            "T_definition": "count_per_sender * P",
        },
        "queue_capacity": 64,
        "tx_processing_delay_ps": 0,
        "rx_processing_delay_ps": 0,
        "channel_delay_ps": 0,
        "rx_filter": "*",
        "metrics_window": "1ms",
        "senders": senders,
        "scenarios": scenarios,
        "input_sha256": {
            path: sha256(content) for path, content in sorted(input_files.items())
        },
    }
    metadata_path = output_dir / "generation.json"
    metadata_path.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return metadata


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="directory for the shared NED network, three configurations, and metadata",
    )
    parser.add_argument(
        "--count-per-sender",
        type=int,
        default=31_250,
        help="periodic request count for each of 32 senders (default: 31250)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        metadata = generate(args.output, args.count_per_sender)
    except (OSError, ValueError, AssertionError) as error:
        raise SystemExit(f"error: {error}") from error
    scenarios = metadata["scenarios"]
    assert isinstance(scenarios, list)
    print(
        f"Generated {len(scenarios)} configurations, "
        f"{metadata['request_count_per_configuration']} requests each, "
        f"under {args.output}"
    )
    for scenario in scenarios:
        assert isinstance(scenario, dict)
        print(
            f"rho={scenario['rho']}: P={scenario['P_ps']}ps, "
            f"T={scenario['T_ps']}ps"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
