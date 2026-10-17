"""Check current guide inventory and this article's historical site evidence."""

from pathlib import Path

from site_inventory import check_site_inventory

ROOT = Path(__file__).resolve().parents[1]


def main():
    check_site_inventory(ROOT, 'ethernet-tas', 'Ethernet TASの送信窓とIFG込みの待ち時間.md')


if __name__ == "__main__":
    main()
