#!/usr/bin/env python3
"""Render repository PlantUML sources, or verify their stored SVG output."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
DIAGRAMS_DIR = REPO_ROOT / "docs" / "diagrams"
START_UML = re.compile(r"^\s*@startuml(?:\s+(\S+))?\s*$", re.MULTILINE)
END_UML = re.compile(r"^\s*@enduml\s*$", re.MULTILINE)


def sources() -> list[Path]:
    return sorted(DIAGRAMS_DIR.rglob("*.puml")) if DIAGRAMS_DIR.is_dir() else []


def validate_source(source: Path) -> None:
    content = source.read_text(encoding="utf-8")
    starts = START_UML.findall(content)
    ends = END_UML.findall(content)
    if len(starts) != 1 or len(ends) != 1:
        raise ValueError(
            f"{source.relative_to(REPO_ROOT)}: exactly one @startuml and @enduml are required"
        )
    match = START_UML.search(content)
    if match.group(1) != source.stem:
        raise ValueError(
            f"{source.relative_to(REPO_ROOT)}: @startuml name must be {source.stem!r}"
        )


def render(source: Path, output_dir: Path) -> Path:
    output_dir.mkdir(parents=True, exist_ok=True)
    command = [
        "plantuml",
        "-charset",
        "UTF-8",
        "-failfast2",
        "-tsvg",
        "-o",
        str(output_dir),
        str(source),
    ]
    subprocess.run(command, cwd=REPO_ROOT, check=True)
    generated = output_dir / f"{source.stem}.svg"
    if not generated.is_file():
        raise OSError(f"PlantUML did not produce {generated}")
    return generated


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Render docs/diagrams/**/*.puml, or verify stored SVGs."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="render into a temporary directory and compare without changing SVGs",
    )
    args = parser.parse_args()
    diagram_sources = sources()
    if not diagram_sources:
        print("No PlantUML sources found under docs/diagrams.")
        return 0

    try:
        for source in diagram_sources:
            validate_source(source)
    except (OSError, UnicodeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    if not args.check:
        try:
            for source in diagram_sources:
                render(source, source.parent)
                print(f"Rendered {source.relative_to(REPO_ROOT)}")
        except subprocess.CalledProcessError as error:
            print(f"error: PlantUML failed with exit status {error.returncode}", file=sys.stderr)
            return error.returncode or 1
        except OSError as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 0

    differences: list[Path] = []
    try:
        with tempfile.TemporaryDirectory(prefix="dir-simulator-diagrams-") as temporary:
            temporary_root = Path(temporary)
            for source in diagram_sources:
                relative_dir = source.parent.relative_to(DIAGRAMS_DIR)
                generated = render(source, temporary_root / relative_dir)
                expected = source.with_suffix(".svg")
                if not expected.is_file() or generated.read_bytes() != expected.read_bytes():
                    differences.append(expected.relative_to(REPO_ROOT))
    except subprocess.CalledProcessError as error:
        print(f"error: PlantUML failed with exit status {error.returncode}", file=sys.stderr)
        return error.returncode or 1
    except OSError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    if differences:
        for path in differences:
            print(f"out of date or missing: {path}", file=sys.stderr)
        return 1
    print(f"Checked {len(diagram_sources)} PlantUML diagram(s).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
