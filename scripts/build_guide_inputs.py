"""Build or check the reproducible public CAN exercise download."""
import argparse
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/can'
OUTPUT = ROOT / 'docs/guide/downloads/can-guide-inputs.zip'


def build(source, output, check):
    files = sorted(p for p in source.rglob('*') if p.is_file())
    if check:
        with zipfile.ZipFile(output) as archive:
            expected = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in files}
            assert set(archive.namelist()) == set(expected), 'ZIP members differ from public sample inputs'
            for name, contents in expected.items():
                assert archive.read(name) == contents, f'Stale downloadable sample: {name}'
        print(f'PASS: {output.name} matches all {len(files)} public input files.')
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path in files:
            info = zipfile.ZipInfo(path.relative_to(ROOT).as_posix(), (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, path.read_bytes())
    print(f'Built {output.name} from {len(files)} files.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    build(SOURCE, OUTPUT, args.check)
    build(ROOT / 'examples/guide/canfd', OUTPUT.with_name('canfd-guide-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-deadline', OUTPUT.with_name('ethernet-deadline-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/gateway-delay', OUTPUT.with_name('gateway-delay-inputs.zip'), args.check)


if __name__ == '__main__':
    main()
