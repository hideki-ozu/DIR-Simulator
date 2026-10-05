"""Build or check the reproducible public CAN exercise download."""
import argparse
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/can'
OUTPUT = ROOT / 'docs/guide/downloads/can-guide-inputs.zip'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    files = sorted(p for p in SOURCE.rglob('*') if p.is_file())
    if args.check:
        with zipfile.ZipFile(OUTPUT) as archive:
            expected = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in files}
            assert set(archive.namelist()) == set(expected), 'ZIP members differ from public sample inputs'
            for name, contents in expected.items():
                assert archive.read(name) == contents, f'Stale downloadable sample: {name}'
        print(f'PASS: download ZIP matches all {len(files)} public input files.')
        return
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(OUTPUT, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path in files:
            info = zipfile.ZipInfo(path.relative_to(ROOT).as_posix(), (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, path.read_bytes())
    print(f'Built public input ZIP from {len(files)} files.')


if __name__ == '__main__':
    main()
