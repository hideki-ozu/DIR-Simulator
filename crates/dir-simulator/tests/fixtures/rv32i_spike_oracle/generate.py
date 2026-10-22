#!/usr/bin/env python3
"""Generate RV32I one-step expectations from a pinned Spike binary's commit log."""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import re
import struct
import subprocess
import sys
from pathlib import Path


BASE = 0x80000000
DATA = 0x80001000
MEMORY_SIZE = 0x2000
SPIKE_COMMIT = "b6a84e1f3a51800d8354489b6493723742abff9a"
SOFTFLOAT_COMMIT = "73e3f397e84d6d2a34bded52b0a77af3f4062ac2"
BASE_OPCODE_NAMES = [
    "LUI", "AUIPC", "JAL", "JALR", "BEQ", "BNE", "BLT", "BGE", "BLTU", "BGEU",
    "LB", "LH", "LW", "LBU", "LHU", "SB", "SH", "SW", "ADDI", "SLTI", "SLTIU",
    "XORI", "ORI", "ANDI", "SLLI", "SRLI", "SRAI", "ADD", "SUB", "SLL", "SLT",
    "SLTU", "XOR", "SRL", "SRA", "OR", "AND", "FENCE", "ECALL", "EBREAK",
]
LOG_RE = re.compile(
    r"^core\s+\d+:\s+\d+\s+(0x[0-9a-fA-F]+)\s+"
    r"\((0x[0-9a-fA-F]+)\)(.*)$"
)
REG_RE = re.compile(r"\bx(\d+)\s+(0x[0-9a-fA-F]+)")
MEM_RE = re.compile(r"\bmem\s+(0x[0-9a-fA-F]+)(?:\s+(0x[0-9a-fA-F]+))?")


def i_type(imm: int, rs1: int, funct3: int, rd: int, opcode: int = 0x13) -> int:
    return ((imm & 0xFFF) << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode


def r_type(funct7: int, rs2: int, rs1: int, funct3: int, rd: int) -> int:
    return (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | 0x33


def s_type(imm: int, rs2: int, rs1: int, funct3: int) -> int:
    n = imm & 0xFFF
    return ((n >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | ((n & 31) << 7) | 0x23


def b_type(imm: int, rs2: int, rs1: int, funct3: int) -> int:
    n = imm & 0x1FFF
    return (
        (((n >> 12) & 1) << 31)
        | (((n >> 5) & 0x3F) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (funct3 << 12)
        | (((n >> 1) & 0xF) << 8)
        | (((n >> 11) & 1) << 7)
        | 0x63
    )


def j_type(imm: int, rd: int) -> int:
    n = imm & 0x1FFFFF
    return (
        (((n >> 20) & 1) << 31)
        | (((n >> 1) & 0x3FF) << 21)
        | (((n >> 11) & 1) << 20)
        | (((n >> 12) & 0xFF) << 12)
        | (rd << 7)
        | 0x6F
    )


def li(rd: int, value: int) -> list[int]:
    value &= 0xFFFFFFFF
    signed_low = value & 0xFFF
    if signed_low & 0x800:
        signed_low -= 0x1000
    upper = ((value - signed_low) >> 12) & 0xFFFFF
    words: list[int] = []
    if upper:
        words.append((upper << 12) | (rd << 7) | 0x37)
    if signed_low or not words:
        words.append(i_type(signed_low, rd if upper else 0, 0, rd))
    return words


def elf32(
    code: list[int],
    data: bytes = bytes(128),
    entry: int = BASE,
    code_base: int = BASE,
    data_base: int = DATA,
) -> bytes:
    code_bytes = b"".join(struct.pack("<I", word) for word in code)
    data_offset = 0x2000
    code_offset = 0x1000
    phoff = 52
    phnum = 2 if data else 1
    shoff = 0x3000
    shstrtab_offset = shoff + 2 * 40
    shstrtab = b"\x00.shstrtab\x00"
    ehdr = struct.pack(
        "<16sHHIIIIIHHHHHH",
        b"\x7fELF\x01\x01\x01" + bytes(9),
        2,
        243,
        1,
        entry,
        phoff,
        shoff,
        0,
        52,
        32,
        phnum,
        40,
        2,
        1,
    )
    phdrs = [
        struct.pack(
            "<IIIIIIII",
            1,
            code_offset,
            code_base,
            code_base,
            len(code_bytes),
            len(code_bytes),
            7,
            0x1000,
        )
    ]
    if data:
        phdrs.append(
            struct.pack(
                "<IIIIIIII", 1, data_offset, data_base, data_base, len(data), len(data), 6, 0x1000
            )
        )
    image = bytearray(max(code_offset + len(code_bytes), data_offset + len(data), shstrtab_offset + len(shstrtab)))
    image[: len(ehdr)] = ehdr
    image[phoff : phoff + len(phdrs) * 32] = b"".join(phdrs)
    image[code_offset : code_offset + len(code_bytes)] = code_bytes
    if data:
        image[data_offset : data_offset + len(data)] = data
    shstr = struct.pack(
        "<IIIIIIIIII", 1, 3, 0, 0, shstrtab_offset, len(shstrtab), 0, 0, 1, 0
    )
    image[shoff + 40 : shoff + 80] = shstr
    image[shstrtab_offset : shstrtab_offset + len(shstrtab)] = shstrtab
    return bytes(image)


def parse_commit_log(path: Path) -> list[dict]:
    result = []
    for line in path.read_text(errors="replace").splitlines():
        match = LOG_RE.match(line)
        if not match:
            continue
        tail = match.group(3)
        regs = [{"rd": int(rd), "value": int(value, 16)} for rd, value in REG_RE.findall(tail)]
        mem_values = []
        for address, value in MEM_RE.findall(tail):
            mem_values.append(int(address, 16))
            if value:
                mem_values.append(int(value, 16))
        result.append(
            {
                "pc": int(match.group(1), 16) & 0xFFFFFFFF,
                "word": int(match.group(2), 16) & 0xFFFFFFFF,
                "register_writes": regs,
                "memory_log_values": mem_values,
            }
        )
    return result


def observe_memory(word: int, log_values: list[int]) -> tuple[list[int], list[dict]]:
    opcode = word & 0x7F
    if opcode == 0x03:
        return log_values, []
    if opcode != 0x23 or len(log_values) < 2:
        return [], []
    funct3 = (word >> 12) & 7
    size = {0: 1, 1: 2, 2: 4}.get(funct3)
    return [], [{"address": log_values[0], "value": log_values[1], "bytes": size}]


def enrich_trace(
    records: list[dict], select_from_pc: int | None = None, initial_data: bytes = bytes(128)
) -> list[dict]:
    regs = [0] * 32
    memory = bytearray(initial_data)
    out = []
    for index, record in enumerate(records):
        word = record["word"]
        pc = record["pc"]
        before = regs.copy()
        writes = record["register_writes"]
        reads, stores = observe_memory(word, record["memory_log_values"])
        memory_bytes = []
        if reads:
            width = {0: 1, 1: 2, 2: 4, 4: 1, 5: 2}[(word >> 12) & 7]
            offset = reads[0] - DATA
            memory_bytes = list(memory[offset : offset + width])
        if select_from_pc is None or pc >= select_from_pc:
            if index + 1 < len(records):
                out.append(
                    {
                        "pc": pc,
                        "word": word,
                        "registers_before": before,
                        "next_pc": records[index + 1]["pc"],
                        "register_writes": writes,
                        "memory_reads": reads,
                        "memory_bytes": memory_bytes,
                        "memory_writes": stores,
                    }
                )
        for write in writes:
            regs[write["rd"]] = write["value"]
        regs[0] = 0
        for store in stores:
            offset = store["address"] - DATA
            for index in range(store["bytes"]):
                memory[offset + index] = (store["value"] >> (index * 8)) & 0xFF
    return out


def registers_before(records: list[dict], index: int) -> list[int]:
    regs = [0] * 32
    for record in records[:index]:
        for write in record["register_writes"]:
            regs[write["rd"]] = write["value"]
        regs[0] = 0
    return regs


def run_spike(spike: Path, image: Path, count: int, log: Path) -> tuple[list[dict], list[str]]:
    command = [
        str(spike),
        "--isa=RV32I",
        "--priv=m",
        f"--instructions={count}",
        "--log-commits",
        "--log",
        str(log),
        f"-m{BASE:#x}:{MEMORY_SIZE:#x}",
        f"--pc={BASE:#x}",
        str(image),
    ]
    completed = subprocess.run(command, text=True, capture_output=True)
    if completed.returncode != 0:
        raise RuntimeError(
            f"Spike failed ({completed.returncode}) for {image.name}:\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return parse_commit_log(log), command


def run_trap_probe(spike: Path, evidence: Path, name: str, target, initial: dict[int, int], steps: int = 1) -> dict:
    initial = {rd: value & 0xFFFFFFFF for rd, value in initial.items() if rd != 0 and value != 0}
    if callable(target):
        initial = dict(initial)
        if name == "jalr_target_misaligned":
            initial[10] = 0
        for _ in range(4):
            init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
            pc = BASE + len(init_words) * 4
            if name == "jalr_target_misaligned":
                initial[10] = (pc + 10) & 0xFFFFFFFF
        init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
        word = target(pc)
    else:
        init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
        pc = BASE + len(init_words) * 4
        word = target
    image = evidence / f"trap-{name}.elf"
    command_file = evidence / f"trap-{name}.commands"
    commands = [f"until pc 0 {pc:x}"]
    commands.extend(f"reg 0 {rd}" for rd in range(32))
    commands.append(f"rs {steps}")
    commands.extend(["reg 0 mcause", "reg 0 mepc", "reg 0 mtval", "q"])
    command_file.write_text("\n".join(commands) + "\n")
    code = init_words + [word, 0x00000013, 0x0000006F]
    image.write_bytes(elf32(code, bytes(128)))
    command = [
        str(spike),
        "-d",
        "--isa=RV32I",
        "--priv=m",
        f"--debug-cmd={command_file}",
        f"-m{BASE:#x}:{MEMORY_SIZE:#x}",
        f"--pc={BASE:#x}",
        str(image),
    ]
    completed = subprocess.run(command, text=True, capture_output=True, timeout=30)
    if completed.returncode != 0:
        raise RuntimeError(
            f"Spike trap probe failed ({completed.returncode}) for {name}:\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    output = (completed.stdout + "\n" + completed.stderr).splitlines()
    numbers = [
        int(match.group(1), 16)
        for line in output
        if (match := re.fullmatch(r"\s*(0x[0-9a-fA-F]+)\s*", line))
    ]
    if len(numbers) < 35:
        raise RuntimeError(f"Spike debug probe {name} returned only {len(numbers)} CSR/register values:\n{completed.stderr}")
    regs = numbers[:32]
    mcause, mepc, mtval = numbers[32:35]
    return {
        "name": name,
        "pc": pc,
        "word": word,
        "registers_before": regs,
        "mcause": mcause,
        "mepc": mepc,
        "mtval": mtval,
        "steps": steps,
        "command": command,
        "debug_commands": commands,
    }


def run_pc_wrap_probe(spike: Path, evidence: Path) -> dict:
    entry = 0xFFFFFFFC
    code_base = 0xFFFFF000
    code = [0x00000013] * 1023 + [i_type(1, 0, 0, 3)]
    image = evidence / "pc-wrap.elf"
    command_file = evidence / "pc-wrap.commands"
    commands = ["rs 1", "pc 0", "reg 0 3", "q"]
    command_file.write_text("\n".join(commands) + "\n")
    image.write_bytes(elf32(code, data=b"", entry=entry, code_base=code_base))
    command = [
        str(spike),
        "-d",
        "--isa=RV32I",
        "--priv=m",
        f"--debug-cmd={command_file}",
        "-m0xfffff000:0x1000",
        f"--pc={entry:#x}",
        str(image),
    ]
    completed = subprocess.run(command, text=True, capture_output=True, timeout=30)
    if completed.returncode != 0:
        raise RuntimeError(f"Spike PC wrap probe failed:\n{completed.stdout}\n{completed.stderr}")
    output = (completed.stdout + "\n" + completed.stderr).splitlines()
    numbers = [
        int(match.group(1), 16)
        for line in output
        if (match := re.fullmatch(r"\s*(0x[0-9a-fA-F]+)\s*", line))
    ]
    if len(numbers) != 2 or numbers[0] != 0 or numbers[1] != 1:
        raise RuntimeError(f"Spike PC wrap probe observed {numbers}, expected [0, 1]:\n{completed.stderr}")
    return {
        "pc": entry,
        "word": code[-1],
        "rd": 3,
        "value": numbers[1],
        "next_pc": numbers[0],
        "command": command,
        "debug_commands": commands,
    }


def build_canonical_cases() -> list[dict]:
    data = bytearray(128)
    data[:8] = bytes([0x80, 0x7F, 0xFE, 0x80, 1, 2, 3, 4])
    data[16:20] = bytes([0xEF, 0xBE, 0xAD, 0xDE])
    cases = []

    def add(name: str, word_or_builder, initial: dict[int, int], data_bytes: bytes = bytes(data)):
        initial = {rd: value & 0xFFFFFFFF for rd, value in initial.items() if rd != 0 and value != 0}
        if callable(word_or_builder) and name == "JALR":
            initial = dict(initial)
            initial[10] = 0
            for _ in range(4):
                init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
                pc = BASE + len(init_words) * 4
                initial[10] = (pc + 9) & 0xFFFFFFFF
            init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
            word = word_or_builder(pc)
        elif callable(word_or_builder):
            init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
            pc = BASE + len(init_words) * 4
            word = word_or_builder(pc)
        else:
            init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
            pc = BASE + len(init_words) * 4
            word = word_or_builder
        target_index = len(init_words)
        code = init_words + [word, 0x00000013, 0x0000006F]
        cases.append(
            {
                "name": name,
                "word": word,
                "pc": pc,
                "code": code,
                "target_index": target_index,
                "instruction_count": target_index + 2,
                "data": data_bytes,
                "initial_registers": initial,
            }
        )

    add("LUI", (0xABCDE << 12) | (3 << 7) | 0x37, {})
    add("AUIPC", (0x12345 << 12) | (3 << 7) | 0x17, {})
    add("JAL", lambda pc: j_type(8, 3), {})
    add("JALR", lambda pc: i_type(0, 10, 0, 3, 0x67), {})

    branch_specs = [
        ("BEQ", 0, 0, 0, True),
        ("BNE", 1, 0, 0xFFFFFFFF, True),
        ("BLT", 4, 0x80000000, 1, True),
        ("BGE", 5, 0x80000000, 1, False),
        ("BLTU", 6, 0xFFFFFFFF, 1, False),
        ("BGEU", 7, 0xFFFFFFFF, 1, True),
    ]
    for name, funct3, lhs, rhs, _ in branch_specs:
        add(name, b_type(8, 2, 1, funct3), {1: lhs, 2: rhs})

    add("LB", i_type(0, 1, 0, 3, 0x03), {1: DATA})
    add("LH", i_type(2, 1, 1, 3, 0x03), {1: DATA})
    add("LW", i_type(16, 1, 2, 3, 0x03), {1: DATA})
    add("LBU", i_type(0, 1, 4, 3, 0x03), {1: DATA})
    add("LHU", i_type(2, 1, 5, 3, 0x03), {1: DATA})
    add("SB", s_type(24, 2, 1, 0), {1: DATA, 2: 0xDEADBEEF})
    add("SH", s_type(26, 2, 1, 1), {1: DATA, 2: 0xDEADBEEF})
    add("SW", s_type(28, 2, 1, 2), {1: DATA, 2: 0xDEADBEEF})

    for name, funct3, imm in [
        ("ADDI", 0, -1),
        ("SLTI", 2, 1),
        ("SLTIU", 3, -1),
        ("XORI", 4, 0x555),
        ("ORI", 6, 0x555),
        ("ANDI", 7, 0x555),
        ("SLLI", 1, 31),
        ("SRLI", 5, 31),
        ("SRAI", 0x205, 31),
    ]:
        add(name, i_type(imm, 1, funct3 & 7, 3) | ((0x20 if funct3 & 0x200 else 0) << 25), {1: 0x80000001})

    for name, funct7, funct3, lhs, rhs in [
        ("ADD", 0, 0, 0xFFFFFFFF, 2),
        ("SUB", 0x20, 0, 0x80000000, 1),
        ("SLL", 0, 1, 1, 31),
        ("SLT", 0, 2, 0x80000000, 1),
        ("SLTU", 0, 3, 0xFFFFFFFF, 1),
        ("XOR", 0, 4, 0xAAAAAAAA, 0x55555555),
        ("SRL", 0, 5, 0x80000000, 31),
        ("SRA", 0x20, 5, 0x80000000, 31),
        ("OR", 0, 6, 0xAAAAAAAA, 0x55555555),
        ("AND", 0, 7, 0xAAAAAAAA, 0x55555555),
    ]:
        add(name, r_type(funct7, 2, 1, funct3, 3), {1: lhs, 2: rhs})
    add("FENCE", 0x0FF0000F, {})
    return cases


def build_seeded_program() -> tuple[list[int], int, int, int]:
    seed = 0x5256333249
    rng = random.Random(seed)
    initial = {rd: rng.getrandbits(32) for rd in range(1, 32)}
    init_words = sum((li(rd, value) for rd, value in sorted(initial.items())), [])
    words = []
    for _ in range(256):
        rd, rs1, rs2 = (rng.randint(1, 31) for _ in range(3))
        if rng.randrange(2):
            f3 = rng.choice([0, 2, 3, 4, 6, 7])
            imm = rng.randrange(-2048, 2048)
            words.append(i_type(imm, rs1, f3, rd))
        else:
            f3 = rng.randrange(8)
            f7 = 0x20 if f3 in (0, 5) and rng.randrange(2) else 0
            words.append(r_type(f7, rs2, rs1, f3, rd))
    code = init_words + words + [0x0000006F]
    return code, len(init_words), seed, len(words)


def build_real_program() -> tuple[list[int], bytes]:
    # Sum four aligned words, including a negative value, then store the result.
    return (
        [
            *li(1, DATA),
            *li(2, 4),
            *li(3, 0),
            i_type(0, 1, 2, 4, 0x03),
            r_type(0, 4, 3, 0, 3),
            i_type(4, 1, 0, 1),
            i_type(-1, 2, 0, 2),
            b_type(-16, 0, 2, 1),
            s_type(0, 3, 1, 2),
            0x0000006F,
        ],
        struct.pack("<IIII", 1, 2, 0xFFFFFFFF, 4) + bytes(112),
    )


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--spike", type=Path, required=True)
    parser.add_argument("--spike-repo", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path(__file__).with_name("oracle.json"))
    parser.add_argument("--evidence-dir", type=Path, required=True)
    args = parser.parse_args()
    spike = args.spike.resolve()
    repo = args.spike_repo.resolve()
    evidence = args.evidence_dir.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    actual_commit = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
    submodule = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD:berkeley-softfloat-3"], text=True).strip()
    if actual_commit != SPIKE_COMMIT or submodule != SOFTFLOAT_COMMIT:
        raise SystemExit(f"wrong Spike source revision: {actual_commit} / {submodule}")

    manifest = {
        "schema": "dir.rv32i.spike-oracle.v1",
        "generator_sha256": sha256(Path(__file__).resolve()),
        "generation_command": [sys.executable, *sys.argv],
        "build": {
            "source_directory": str(repo),
            "build_directory": "/tmp/dir-rv32i-oracle-build",
            "configure_command": [
                str(repo / "configure"),
                "--prefix=/tmp/dir-rv32i-oracle-install",
                "--without-boost",
                "--without-boost-asio",
                "--without-boost-regex",
            ],
            "make_command": ["make", "-j4"],
            "build_log": "/tmp/dir-remaining-implementation-20261021-evidence/spike-build2.log",
            "build_environment": {
                "PATH_prefix": "/tmp/dir-rv32i-oracle-tools/usr/bin",
                "LD_LIBRARY_PATH_prefix": "/tmp/dir-rv32i-oracle-tools/usr/lib/x86_64-linux-gnu",
            },
        },
        "oracle": {
            "name": "Spike",
            "commit": actual_commit,
            "softfloat_commit": submodule,
            "binary_sha256": sha256(spike),
            "version_line": (
                lambda result: (result.stderr + result.stdout).splitlines()[0]
            )(subprocess.run([str(spike), "--help"], text=True, capture_output=True, check=False)),
            "source_sha256": {
                rel: sha256(repo / rel)
                for rel in [
                    "spike_main/spike.cc",
                    "riscv/execute.cc",
                    "riscv/processor.cc",
                    "riscv/processor.h",
                    "riscv/mmu.cc",
                    "riscv/mmu.h",
                    "riscv/sim.cc",
                    "riscv/interactive.cc",
                    "riscv/encoding.h",
                    "fesvr/elfloader.cc",
                    *(f"riscv/insns/{name.lower()}.h" for name in BASE_OPCODE_NAMES),
                ]
            },
        },
        "canonical_instruction_cases": [],
        "seeded_trace": {"seed": None, "cases": []},
        "real_program_trace": {"description": "four-word sum loop; result stored at DATA+16", "result": 6, "cases": []},
        "trap_probes": [],
    }
    command_evidence = []

    for case in build_canonical_cases():
        label = case["name"].lower()
        image = evidence / f"canonical-{label}.elf"
        log = evidence / f"canonical-{label}.log"
        image.write_bytes(elf32(case["code"], case["data"]))
        records, command = run_spike(spike, image, case["instruction_count"], log)
        target_pc = case["pc"]
        target_pos = next(i for i, record in enumerate(records) if record["pc"] == target_pc)
        if target_pos + 1 >= len(records):
            raise RuntimeError(f"no successor commit after {case['name']} at {target_pc:#x}")
        target = records[target_pos]
        reads, stores = observe_memory(case["word"], target["memory_log_values"])
        manifest["canonical_instruction_cases"].append(
            {
                "name": case["name"],
                "pc": target_pc,
                "word": case["word"],
                "registers_before": registers_before(records, target_pos),
                "next_pc": records[target_pos + 1]["pc"],
                "register_writes": target["register_writes"],
                "memory_reads": reads,
                "memory_bytes": (
                    list(case["data"][reads[0] - DATA : reads[0] - DATA + {0: 1, 1: 2, 2: 4, 4: 1, 5: 2}[(case["word"] >> 12) & 7]])
                    if reads
                    else []
                ),
                "memory_writes": stores,
            }
        )
        command_evidence.append({"case": case["name"], "command": command})

    random_code, setup_count, seed, random_count = build_seeded_program()
    random_image = evidence / "seeded-multiop.elf"
    random_log = evidence / "seeded-multiop.log"
    random_image.write_bytes(elf32(random_code, bytes(128)))
    records, command = run_spike(spike, random_image, len(random_code), random_log)
    start_pc = BASE + setup_count * 4
    random_trace = enrich_trace(records, start_pc)
    if len(random_trace) != random_count:
        raise RuntimeError(f"seeded trace expected {random_count} instructions, got {len(random_trace)}")
    manifest["seeded_trace"] = {"seed": seed, "cases": random_trace}
    command_evidence.append({"case": "seeded-multiop", "command": command})

    real_code, real_data = build_real_program()
    real_image = evidence / "real-program.elf"
    real_log = evidence / "real-program.log"
    real_image.write_bytes(elf32(real_code, real_data))
    records, command = run_spike(spike, real_image, 100, real_log)
    real_trace = enrich_trace(records, initial_data=real_data)
    # Keep the architectural execution through the result store and omit the terminal self-loop.
    store_pc = BASE + (len(real_code) - 2) * 4
    real_trace = [case for case in real_trace if case["pc"] <= store_pc]
    result_writes = [
        write
        for case in real_trace
        if case["pc"] == store_pc
        for write in case["memory_writes"]
    ]
    if not result_writes or result_writes[-1]["value"] != 6:
        raise RuntimeError(f"real program did not store checksum 6: {result_writes}")
    manifest["real_program_trace"]["cases"] = real_trace
    command_evidence.append({"case": "real-program", "command": command})

    manifest["pc_wrap_probe"] = run_pc_wrap_probe(spike, evidence)
    command_evidence.append(
        {
            "case": "pc-wrap",
            "command": manifest["pc_wrap_probe"]["command"],
            "debug_commands": manifest["pc_wrap_probe"]["debug_commands"],
        }
    )

    traps = [
        ("illegal_instruction", 0xFFFFFFFF, {}, 1),
        ("environment_call", 0x00000073, {}, 1),
        ("breakpoint", 0x00100073, {}, 1),
        ("jal_target_misaligned", j_type(2, 3), {}, 1),
        ("jalr_target_misaligned", lambda pc: i_type(0, 10, 0, 3, 0x67), {}, 1),
        ("load_address_misaligned", i_type(0, 1, 2, 3, 0x03), {1: DATA + 1}, 1),
        ("store_address_misaligned", s_type(0, 2, 1, 2), {1: DATA + 1, 2: 0xDEADBEEF}, 1),
        ("load_access_fault", i_type(0, 1, 2, 3, 0x03), {1: 0x10000000}, 1),
        ("store_access_fault", s_type(0, 2, 1, 2), {1: 0x10000000, 2: 0xDEADBEEF}, 1),
        ("instruction_access_fault", lambda pc: i_type(0, 10, 0, 3, 0x67), {10: 0x10000000}, 2),
    ]
    for name, word, initial, steps in traps:
        probe = run_trap_probe(spike, evidence, name, word, initial, steps)
        probe["cause"] = {
            0: "instruction_address_misaligned",
            1: "instruction_access_fault",
            2: "illegal_instruction",
            3: "breakpoint",
            4: "load_address_misaligned",
            5: "load_access_fault",
            6: "store_address_misaligned",
            7: "store_access_fault",
            11: "environment_call",
        }.get(probe["mcause"], f"unknown_{probe['mcause']}")
        expected_cause = name
        if name == "jal_target_misaligned" or name == "jalr_target_misaligned":
            expected_cause = "instruction_address_misaligned"
        if probe["cause"] != expected_cause:
            raise RuntimeError(f"Spike trap probe {name} observed {probe['cause']} ({probe})")
        probe["stage"] = (
            "memory_access"
            if name in ("load_access_fault", "store_access_fault")
            else "instruction_fetch"
            if name == "instruction_access_fault"
            else "cpu_decode"
        )
        probe["expected_mepc"] = 0x10000000 if name == "instruction_access_fault" else probe["pc"]
        probe["expected_tval"] = {
            "illegal_instruction": probe["word"],
            "environment_call": 0,
            "breakpoint": probe["pc"],
            "jal_target_misaligned": probe["pc"] + 2,
            "jalr_target_misaligned": probe["pc"] + 10,
            "load_address_misaligned": DATA + 1,
            "store_address_misaligned": DATA + 1,
            "load_access_fault": 0x10000000,
            "store_access_fault": 0x10000000,
            "instruction_access_fault": 0x10000000,
        }[name]
        if probe["mepc"] != probe["expected_mepc"] or probe["mtval"] != probe["expected_tval"]:
            raise RuntimeError(f"Spike trap probe {name} had unexpected mepc/mtval: {probe}")
        manifest["trap_probes"].append(probe)
        command_evidence.append({"case": f"trap:{name}", "command": probe["command"], "debug_commands": probe["debug_commands"]})

    manifest["oracle"]["commands"] = command_evidence
    manifest["summary"] = {
        "canonical_instruction_case_count": len(manifest["canonical_instruction_cases"]),
        "base_opcode_count": len(manifest["canonical_instruction_cases"]) + 2,
        "covered_base_opcodes": BASE_OPCODE_NAMES,
        "seeded_instruction_count": len(manifest["seeded_trace"]["cases"]),
        "real_program_instruction_count": len(manifest["real_program_trace"]["cases"]),
        "coverage_names": [case["name"] for case in manifest["canonical_instruction_cases"]],
    }
    args.output.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {args.output}: {manifest['summary']}")
    print(f"Spike binary SHA-256: {manifest['oracle']['binary_sha256']}")
    print(f"Spike source commit: {actual_commit}; SoftFloat: {submodule}")


if __name__ == "__main__":
    main()
