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
    build(ROOT / 'examples/guide/sram-ports', OUTPUT.with_name('sram-ports-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/mailbox-capacity', OUTPUT.with_name('mailbox-capacity-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/dma-notification', OUTPUT.with_name('dma-notification-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ddr-refresh', OUTPUT.with_name('ddr-refresh-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/axi-strobes', OUTPUT.with_name('axi-strobes-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/shared-roles', OUTPUT.with_name('shared-roles-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/soc-arbitration', OUTPUT.with_name('soc-arbitration-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ahb-wait-error', OUTPUT.with_name('ahb-wait-error-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/noc-input-fifo', OUTPUT.with_name('noc-input-fifo-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-priority', OUTPUT.with_name('ethernet-priority-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-vlan', OUTPUT.with_name('ethernet-vlan-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-tas', OUTPUT.with_name('ethernet-tas-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-multicast', OUTPUT.with_name('ethernet-multicast-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-cbs', OUTPUT.with_name('ethernet-cbs-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-psfp', OUTPUT.with_name('ethernet-psfp-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-learning', OUTPUT.with_name('ethernet-learning-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-link-control', OUTPUT.with_name('ethernet-link-control-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-source-filter', OUTPUT.with_name('ethernet-source-filter-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-vid-registration', OUTPUT.with_name('ethernet-vid-registration-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-stp-cost', OUTPUT.with_name('ethernet-stp-cost-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-preemption-fragments', OUTPUT.with_name('ethernet-preemption-fragments-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-redundancy-history', OUTPUT.with_name('ethernet-redundancy-history-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/ethernet-psfp-gate', OUTPUT.with_name('ethernet-psfp-gate-inputs.zip'), args.check)
    build(ROOT / 'examples/guide/metric-windows', OUTPUT.with_name('metric-windows-inputs.zip'), args.check)


if __name__ == '__main__':
    main()
