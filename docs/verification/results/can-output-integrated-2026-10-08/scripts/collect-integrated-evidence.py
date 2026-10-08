#!/usr/bin/env python3
"""Collect completed integrated CAN output evidence without running workloads."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import difflib
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import statistics
import sys
import tarfile

ROOT = Path('/tmp/dir-opt-integrated-2026-10-08')
REPO = Path('/home/hideki/DIR-Simulator')
RESULTS = REPO / 'docs/verification/results'
STEM = 'can-output-integrated-2026-10-08'
ALLOWED = {
    'crates/dir-simulator/src/output.rs',
    *('crates/dir-simulator/src/output/' + name for name in (
        'disk_sort.rs', 'stream.rs', 'json.rs', 'csv.rs', 'publish.rs',
        'contribution_sort.rs')),
}
CASES = ['n16000-rho030', 'n3200-rho090', 'n3200-rho120']


def require(condition, detail):
    if not condition:
        raise RuntimeError(detail)


def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def read(path):
    require(path.is_file() and not path.is_symlink(), f'Missing regular file: {path}')
    return json.loads(path.read_text())


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def safe(raw):
    p = PurePosixPath(raw)
    require(not p.is_absolute() and p.parts and '..' not in p.parts, f'Unsafe path: {raw}')
    return p.as_posix()


def inventory_source(root):
    paths = [*root.joinpath('crates').rglob('*'), *(root / n for n in (
        'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'))]
    return {str(p.relative_to(root)): sha(p) for p in sorted(paths)
            if p.is_file() and not p.is_symlink()}


def load_controller_module():
    spec = importlib.util.spec_from_file_location('integrated_measure', ROOT / 'measure-integrated.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controller', type=Path, required=True)
    args = parser.parse_args()
    controller_path = args.controller.resolve(strict=True)
    controller = read(controller_path)
    require(controller.get('status') == 'complete' and controller.get('attempt_count') == 24,
            'Measurements must be complete with 24 attempts')
    require([x['id'] for x in controller['comparisons']] == CASES, 'Unexpected cases')
    module = load_controller_module()
    plan = read(ROOT / 'measurement-plan.json')
    baseline = module.validate_baseline(plan)
    integrated = module.validate_integrated(plan)
    require(baseline == controller['baseline'], 'Baseline association changed')
    require(integrated == controller['integrated'], 'Integrated association changed')
    require(sha(ROOT / 'measurement-plan.json') == controller['measurement_plan']['sha256'],
            'Measurement plan changed')
    require(sha(ROOT / 'measure-integrated.py') == controller['measurement_controller']['sha256'],
            'Measurement script changed')
    before = baseline['source_sha256']
    after = integrated['source_sha256']
    changes = sorted(p for p in set(before) | set(after) if before.get(p) != after.get(p))
    require(set(changes) == ALLOWED, f'Unexpected production changes: {changes}')
    require(len(before) == 183 and len(after) == 184, 'Unexpected source inventory counts')
    gate = read(ROOT / 'gates/latest.json')
    for command in gate['commands']:
        require(sha(Path(command['log'])) == command['log_sha256'], 'Integrated gate log changed')
    test_log = Path(gate['commands'][2]['log']).read_text()
    counts = re.findall(r'test result:.*?(\d+) passed; (\d+) failed; (\d+) ignored;', test_log)
    require(gate['tests'] == {
        'passed': sum(int(x[0]) for x in counts), 'failed': sum(int(x[1]) for x in counts),
        'ignored': sum(int(x[2]) for x in counts), 'suites': len(counts)}, 'Test log summary mismatch')

    comparisons = []
    attempt_count = 0
    for case, entry in zip(controller['cases'], controller['comparisons']):
        require(entry['status'] == 'complete', 'Incomplete case')
        module.pinned_case_record(case)
        validated = module.validate_attempts(Path(entry['helper_report_path']), case,
                                            {n: controller[n]['binary'] for n in ('baseline', 'integrated')})
        require(validated == entry['validation'], 'Controller summary differs from raw attempts')
        comparison = read(Path(entry['helper_report_path']))
        for attempt in comparison['attempts']:
            local = Path(attempt['evidence_directory'])
            original = read(local / 'attempt.json')
            records = [json.loads(line) for line in (local / 'gnu-time.txt').read_text().splitlines()
                       if line.startswith('{')]
            require(records, 'Missing raw GNU time measurement')
            raw = records[-1]
            require(raw['wall_seconds'] == attempt['measurements']['wall_seconds']
                    == original['measurements']['wall_seconds'], 'Raw wall mismatch')
            require(raw['max_rss_kib'] * 1024 == attempt['max_rss_bytes']
                    == original['max_rss_bytes'], 'Raw RSS mismatch')
            require(original['files'] == attempt['files'] and original['deterministic'] == attempt['deterministic'],
                    'Raw attempt output verification mismatch')
            attempt_count += 1
        comparisons.append(entry)
    require(attempt_count == 24, 'Missing raw attempts')

    destination = RESULTS / STEM
    json_path = RESULTS / (STEM + '.json')
    md_path = RESULTS / (STEM + '.md')
    require(not destination.exists() and not json_path.exists() and not md_path.exists(),
            'Final evidence destination already exists')
    stage = RESULTS / ('.' + STEM + '-collecting')
    require(not stage.exists(), 'Collection stage already exists')
    stage.mkdir(parents=True)
    inventory = []

    def add(source, relative, kind):
        relative = safe(relative)
        target = stage / relative
        require(source.is_file() and not source.is_symlink(), f'Unsafe or missing input: {source}')
        require(not target.exists(), f'Duplicate support target: {relative}')
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        require(sha(source) == sha(target), f'Copied hash mismatch: {source}')
        row = {'kind': kind, 'original_local_path': str(source.resolve()),
               'support_relative_path': relative, 'bytes': target.stat().st_size, 'sha256': sha(target)}
        inventory.append(row)
        return row

    def generated(data, relative, kind, origins):
        target = stage / safe(relative)
        require(not target.exists(), f'Duplicate generated target: {relative}')
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        row = {'kind': kind, 'original_local_path': None, 'generated_from': origins,
               'support_relative_path': relative, 'bytes': len(data), 'sha256': sha(target)}
        inventory.append(row)
        return row

    try:
        add(ROOT / 'before.json', 'before.json', 'frozen_baseline_manifest')
        for relative, expected in sorted(before.items()):
            row = add(ROOT / 'source-before' / relative, 'source-before/' + relative, 'frozen_source')
            require(row['sha256'] == expected, 'Baseline source hash mismatch')
        patch_parts = []
        change_records = []
        for relative in changes:
            original = ROOT / 'source-before' / relative
            current = REPO / relative
            row = add(current, 'source-changes/files/' + relative, 'integrated_changed_source')
            old_text = original.read_text().splitlines(keepends=True) if original.is_file() else []
            new_text = current.read_text().splitlines(keepends=True)
            patch_parts.extend(difflib.unified_diff(old_text, new_text,
                fromfile='a/' + relative if original.is_file() else '/dev/null',
                tofile='b/' + relative))
            change_records.append({'path': relative, 'before_sha256': before.get(relative),
                                   'after_sha256': after[relative],
                                   'support_relative_path': row['support_relative_path']})
        patch = generated(''.join(patch_parts).encode(), 'source-changes/integrated.patch',
                          'integrated_source_patch', [str(ROOT / 'source-before'), str(REPO)])
        for path in sorted((ROOT / 'gates').rglob('*')):
            if path.is_file() and not path.is_symlink():
                add(path, 'gates/integrated/' + str(path.relative_to(ROOT / 'gates')), 'integrated_gate_evidence')
        producer_path = Path(baseline['producer_gate']['path'])
        producer = read(producer_path)
        add(producer_path, 'gates/baseline/producer-latest.json', 'baseline_producer_gate')
        for i, command in enumerate(producer['commands']):
            p = Path(command['log'])
            require(sha(p) == command['log_sha256'], 'Baseline producer log changed')
            add(p, f'gates/baseline/{i}.log', 'baseline_producer_gate_log')
        for path in sorted(controller_path.parent.rglob('*')):
            if path.is_file():
                require(not path.is_symlink() and 'output' not in path.relative_to(controller_path.parent).parts,
                        'Large output or symlink remains in measurement evidence')
                require(path.stat().st_size < 10 * 1024 * 1024, 'Unexpected large evidence file')
                add(path, 'measurements/' + str(path.relative_to(controller_path.parent)), 'measurement_evidence')
        for filename in ('measure-integrated.py', 'measurement-plan.json', 'gate-integrated.py',
                         'collect-integrated-evidence.py'):
            add(ROOT / filename, 'scripts/' + filename, 'reproduction_controller')
        for filename in ('measure_can_export.py', 'measure_can_million.py', 'generate_can_million.py'):
            source = REPO / 'scripts/performance' / filename
            if source.is_file():
                add(source, 'scripts/performance/' + filename, 'reproduction_helper')
        input_sets = {}
        for case in controller['cases']:
            parent = Path(case['config']).parent
            if parent.name in input_sets:
                continue
            generation = parent / 'generation.json'
            g = read(generation)
            add(generation, 'inputs/' + parent.name + '/generation.json', 'input_generation_manifest')
            for relative, expected in g['input_sha256'].items():
                row = add(parent / safe(relative), 'inputs/' + parent.name + '/' + safe(relative), 'pinned_input')
                require(row['sha256'] == expected, 'Pinned input changed')
            input_sets[parent.name] = g
        old_json = RESULTS / 'can-output-candidates-2026-10-08.json'
        old_report = read(old_json)
        add(old_json, 'references/can-output-candidates-2026-10-08.json', 'individual_reference_report')
        archive_original = REPO / old_report['inventory_base_directory'] / 'source-snapshots/baseline-source.tar.gz'
        old_archive_record = next(x for x in old_report['support_files']
                                  if x['support_relative_path'] == 'source-snapshots/baseline-source.tar.gz')
        require(sha(archive_original) == old_archive_record['sha256'], 'Baseline reproduction snapshot changed')
        archive_record = add(archive_original, 'source-snapshots/baseline-source.tar.gz', 'baseline_reproduction_snapshot')
        with tarfile.open(archive_original, 'r:gz') as archive:
            members = {m.name: m for m in archive.getmembers() if m.isfile()}
            for relative, expected in before.items():
                require(relative in members, 'Missing baseline archive source')
                require(hashlib.sha256(archive.extractfile(members[relative]).read()).hexdigest() == expected,
                        'Baseline archive source mismatch')
            for relative, expected in gate['build_documents_sha256'].items():
                require(relative in members, 'Missing build document in baseline archive')
                require(hashlib.sha256(archive.extractfile(members[relative]).read()).hexdigest() == expected,
                        'Build document differs from baseline archive')
        all_specs = {str(p.relative_to(REPO)): sha(p) for p in (REPO / 'docs/specs').rglob('*') if p.is_file()}
        require(all_specs == gate['build_documents_sha256'], 'Build documents changed after gates')
        for p in sorted((ROOT / 'docs-before').rglob('*')):
            if p.is_file():
                add(p, 'docs-before/' + str(p.relative_to(ROOT / 'docs-before')) + '.txt', 'preintegration_document')
        for relative in ('docs/design/CAN出力最適化詳細設計書.md', 'docs/design/結果処理詳細設計書.md',
                         'docs/verification/cases/結果処理検証仕様書.md'):
            add(REPO / relative, 'docs-integrated/' + relative + '.txt', 'integration_design_document')
        inventory.sort(key=lambda x: x['support_relative_path'])
        require(len(inventory) == len({x['support_relative_path'] for x in inventory}), 'Duplicate inventory')
        for row in inventory:
            path = stage / row['support_relative_path']
            require(path.stat().st_size == row['bytes'] and sha(path) == row['sha256'], 'Final inventory mismatch')
        report = {
            'schema_version': 1, 'document_id': STEM,
            'generated_at_utc': datetime.now(timezone.utc).isoformat(),
            'status': 'implemented_gated_measured',
            'scope': 'All five internal CAN/Gateway output optimizations integrated; scaled-input comparison only.',
            'baseline': baseline, 'integrated': integrated, 'gate_tests': gate['tests'],
            'build_documents_sha256': gate['build_documents_sha256'],
            'controller': {'original_local_path': str(controller_path), 'sha256': sha(controller_path),
                           'support_relative_path': 'measurements/controller.json'},
            'cases': controller['cases'], 'comparison_policy': controller['comparison_policy'],
            'comparisons': comparisons, 'source_changes': changes,
            'source_change_records': change_records, 'source_patch': patch,
            'baseline_reproduction_snapshot': {**archive_record,
                'archived_regular_files': old_archive_record['archived_regular_files'],
                'exclusions': old_archive_record['exclusions'],
                'reproduction': 'Extract baseline snapshot, apply integrated.patch; same locked toolchain, build documents and fixtures. Git commit provenance is from the original checkout; binaries are held only in local /tmp.'},
            'validation': {'completed_attempts': attempt_count, 'warmups': 6, 'measurements': 18,
                'output_manifest_files_verified': attempt_count * 4,
                'normalized_outputs_equal_case_count': 3, 'diagnostics_equal_case_count': 3,
                'raw_gnu_time_records_match': True,
                'source_inventory_before': len(before), 'source_inventory_integrated': len(after),
                'source_changes_exactly_authorized_paths': True,
                'unrelated_source_files_unchanged': True,
                'large_outputs_removed_after_verification': True,
                'manifest_evidence': 'All four original declaration entries and observed hash/size pairs remain in attempt.json.files; full original manifest files and result outputs were removed.',
                'full_million_request_verdict': False,
                'os_flush_close_failure_injection': False},
            'support_directory': STEM, 'inventory_base_directory': STEM, 'support_files': inventory,
        }
        stage.rename(destination)
        save(json_path, report)
        lines = [
            '# CAN出力最適化①〜⑤の製品統合・実測記録（2026-10-08）', '',
            '文書ID：`' + STEM + '`', '',
            '①バイナリ外部ソート、②観測点・時間窓の2系列マージ、③JSON／CSVの単一走査、④固定長寄与・Timeline添字参照、⑤Stats添字参照を製品コードへ統合した。[詳細設計](../../design/CAN出力最適化詳細設計書.md)と[検証仕様](../cases/結果処理検証仕様書.md#dir-test-0117)も更新した。公開schema・指標値・行順・採番の契約を維持する。', '',
            '## 統合版の直接比較', '',
            '同じ固定基準版のバイナリと入力を使い、今回の統合版と改めて比較した。個別試作の短縮率を加算していない。各条件・各版はウォームアップ1回と本測定3回で、実行順をroundごとに反転し、全24実行を逐次実行した。比較中にビルド・CPUプロファイリング・別のベンチマークを重ねていない。GNU timeのwallはCLIの準備から出力完了まで、最大RSSは同じCLIプロセスの値である。出力hash検証時間はwallの外にある。', '',
            '時間は3測定の中央値、RSSは3測定の最大値を示す。', '',
            '| 条件 | 基準wall | 統合wall | 時間短縮 | 基準最大RSS | 統合最大RSS |',
            '| --- | ---: | ---: | ---: | ---: | ---: |',
        ]
        labels = {'n16000-rho030': '16,000要求・rho 0.30', 'n3200-rho090': '3,200要求・rho 0.90',
                  'n3200-rho120': '3,200要求・rho 1.20'}
        for entry in comparisons:
            v = entry['validation']; stats = v['summaries_recomputed_from_gnu_time_attempts']
            b, i = stats['baseline'], stats['integrated']
            lines.append(f"| {labels[entry['id']]} | {b['wall_seconds']['median']:.2f}秒 | {i['wall_seconds']['median']:.2f}秒 | {v['integrated_minus_baseline']['wall_reduction_percent']:.1f}% | {b['max_rss_bytes']['maximum']/1048576:.2f} MiB | {i['max_rss_bytes']['maximum']/1048576:.2f} MiB |")
        lines += ['', '| 条件 | 基準wallの3生値 | 統合wallの3生値 |', '| --- | --- | --- |']
        for entry in comparisons:
            stats = entry['validation']['summaries_recomputed_from_gnu_time_attempts']
            values = [', '.join(f'{x:.2f}' for x in stats[n]['wall_seconds']['values']) for n in ('baseline', 'integrated')]
            lines.append(f"| {labels[entry['id']]} | {values[0]} | {values[1]} |")
        lines += ['', '生RSS、範囲、中央値差、測定roundを対応付けた差はJSONに記録した。3測定から微小差の統計的有意性を断定しない。', '',
            '## 正しさと公開処理', '',
            f"Rust 1.85.0でfmt、locked/offline workspace Clippy（warnings禁止）、locked/offline workspace test、release buildが通過した。統合版は{gate['tests']['passed']} passed／0 failed／0 ignored、{gate['tests']['suites']} suites。個別案の試験に加え、バイナリ形式と遅い全行fallback、同一キーの点優先と再走査、直接／fallback双方の生成frame長上限、paired出力の後半行破損・cleanup、複数バスと無通信対象におけるStats／Timelineの対応を検証した。材料化した既存結果とのJSON simulation／CSV比較も通過した。", '',
            '全24実行で各4ファイル、計96ファイルの実byte数・SHA-256をmanifest宣言値と照合した。条件内の8実行すべてでsimulation JSON、run_idを正規化したevents.csv／summary.csv、およびdiagnostics.jsonlのhashが一致した。実行ID・日時・ビルド情報を含むresults.json全体のraw hashは版ごとに変わる。', '',
            '静的統合レビューで、整列済み直接経路とfallbackへの生成frame上限の伝播、fallbackによる保存済み全行の再投入、集計の非再実行、完全キーの点優先、寄与のtime＋tie順、Timeline IDとStats slotの分離、両writer完了後の登録とmanifest最終公開を確認した。OSのflush／close自体に障害を注入した証明は今回の追加試験に含めない。', '',
            '## ソース対応と証跡', '',
            f"基準183ファイルと統合184ファイルのhash mapを固定し、変更は出力層の7ファイル（うちcontribution_sort.rsは追加）に限定した。他の製品ソースは今回の開始時点と一致する。イベント実行・入力・台帳生成を変更していない。測定前後のソース・バイナリ・入力・計測スクリプトのpinを照合した。統合CLI SHA-256は`{integrated['binary']['sha256']}`、基準CLIは`{baseline['binary']['sha256']}`。", '',
            '[機械可読記録](' + STEM + '.json)と同名supportディレクトリにgateログ、測定controller、全24実行のGNU time／attempt／サンプル、入力生成記録と入力、基準ソース、統合差分とpatch、再現用基準snapshotを保存した。`support_relative_path`は`inventory_base_directory`から解決し、`original_local_path`は保存元のローカル位置を示す。実行バイナリはローカル`/tmp/dir-opt-integrated-2026-10-08/bin/`に保持し、製品リポジトリへコピーしていない。', '',
            '大きな結果ファイルと元のmanifestファイルは検証後に削除した。元の4ファイル分のmanifest宣言entryと観測hash／byte数はattempt記録内に保存している。基準snapshotと統合patchを使って測定ソースを復元できる。既存の個別試作記録は当時の製品未適用状態を表す履歴として維持する。', '',
            '## 検証範囲', '',
            '①〜⑤の併用効果を今回直接測定した。フル100万要求の完走・120秒目標・全体peak RSSの受け入れは今回実施していない。小規模測定の短縮率を100万要求へ外挿しない。', '',
        ]
        md_path.write_text('\n'.join(lines))
        print(json.dumps({'json': str(json_path), 'markdown': str(md_path),
                          'support_files': len(inventory), 'attempts': attempt_count}, ensure_ascii=False))
    except BaseException:
        if stage.exists():
            shutil.rmtree(stage)
        raise


if __name__ == '__main__':
    main()
