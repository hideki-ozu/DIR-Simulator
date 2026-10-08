#!/usr/bin/env python3
"""Archive completed million-request observations and source provenance."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess

ROOT = Path('/tmp/dir-million-integrated-2026-10-08')
REPO = Path('/home/hideki/DIR-Simulator')
STEM = 'can-million-integrated-2026-10-08'
RESULTS = REPO / 'docs/verification/results'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def main():
    record = json.loads((ROOT / 'measurement.json').read_text())
    before = json.loads((ROOT / 'before.json').read_text())
    proof = json.loads((ROOT / 'independent-verification.json').read_text())
    assert record['status'] == 'complete' and proof['status'] == 'passed'
    assert len(record['conditions']) == 3
    support = RESULTS / STEM
    report_path = RESULTS / (STEM + '.json')
    md_path = RESULTS / (STEM + '.md')
    assert not support.exists() and not report_path.exists() and not md_path.exists()
    support.mkdir(parents=True)
    inventory = []

    def add(source, relative, kind):
        assert source.is_file() and not source.is_symlink()
        dest = support / relative
        assert not dest.exists()
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, dest)
        assert sha(source) == sha(dest)
        row = {'kind': kind, 'original_local_path': str(source), 'support_relative_path': relative,
               'bytes': dest.stat().st_size, 'sha256': sha(dest)}
        inventory.append(row)
        return row

    for filename in ('before.json', 'measurement.json', 'producer-gate.json', 'run-probes.py',
                     'collect-evidence.py', 'verify-observations.py', 'independent-verification.json'):
        add(ROOT / filename, filename, 'measurement_and_verification_controller')
    for folder in ('measurements', 'source', 'inputs', 'helpers', 'docs-before'):
        for source in sorted((ROOT / folder).rglob('*')):
            if not source.is_file() or '__pycache__' in source.parts:
                continue
            assert not source.is_symlink()
            rel = str(source.relative_to(ROOT))
            if source.suffix == '.md':
                rel += '.txt'
            add(source, rel, folder + '_evidence')
    for relative, expected in before['build_documents_sha256'].items():
        source = REPO / relative
        assert sha(source) == expected
        add(source, 'build-documents/' + relative + '.txt', 'unchanged_build_document')
    producer = json.loads((ROOT / 'producer-gate.json').read_text())
    for i, command in enumerate(producer['commands']):
        source = Path(command['log'])
        assert sha(source) == command['log_sha256']
        add(source, f'gates/{i}.log', 'producer_gate_log')
    environment = {'uname': list(os.uname()), 'os_release': Path('/etc/os-release').read_text(),
                   'cpuinfo': Path('/proc/cpuinfo').read_text(), 'meminfo': before['initial_memory_info'],
                   'rustc': subprocess.check_output(['/home/hideki/.cargo/bin/rustc', '-Vv'], text=True),
                   'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
                   'git_dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=REPO))}
    report = {
        'schema_version': 1, 'document_id': STEM, 'series': 'WSL2 integrated completion observations',
        'status': 'completed_probes_verified',
        'completed_product_runs': sum(x['completed'] for c in record['conditions'] for x in c['attempts']),
        'observed_time_target_met': all(c['attempts'][0]['completion_wall_target_met'] is True for c in record['conditions']),
        'observed_rss_target_met': all(c['attempts'][0]['full_process_rss_target_met'] is True for c in record['conditions']),
        'formal_protocol_complete': all(c.get('formal_evaluation') is not None and c['formal_evaluation']['complete_measurement_count'] == 3 for c in record['conditions']),
        'scope': record['qualification'], 'started_at_utc': record['started_at_utc'],
        'finished_at_utc': record['finished_at_utc'], 'binary': before['binary'],
        'source_sha256': before['source_sha256'], 'build_documents_sha256': before['build_documents_sha256'],
        'producer_gate': before['producer_gate'], 'targets': before['targets'],
        'conditions': record['conditions'], 'environment': environment,
        'generation': json.loads((ROOT / 'inputs/generation.json').read_text()),
        'resource_guards': {'address_space_limit_bytes': 26 * 1024**3,
                            'rss_stop_bytes': 24 * 1024**3,
                            'minimum_available_bytes': 3 * 1024**3,
                            'wall_observation_limit_seconds': 1200},
        'probe_policy': before['probe_policy'], 'independent_verification': proof,
        'prior_integrated_report': before['prior_integrated_report'],
        'native_baseline_verdict': 'unverified',
        'support_directory': STEM, 'inventory_base_directory': STEM,
        'support_files': sorted(inventory, key=lambda x:x['support_relative_path']),
    }
    save(report_path, report)
    lines = ['# 100万要求・出力最適化統合後の完走実測（2026-10-08）', '',
             '文書ID：`' + STEM + '`', '',
             '①〜⑤を適用した製品版で、規定の100万要求を3負荷条件それぞれ完走させ、出力確定までの時間と同一プロセスの最大RSSを測定した。目標は120秒以内・2 GiB以下である。WSL2の追試として記録し、native基準機とは別系列にする。', '',
             '## 実測結果', '',
             '| 負荷 | 完走回数 | 出力確定まで | 最大RSS | イベント処理のみ | 出力サイズ |',
             '| --- | ---: | ---: | ---: | ---: | ---: |']
    for condition in record['conditions']:
        first = condition['attempts'][0]
        bits = first['measurements']['wall_seconds']
        event = first.get('event_processing_wall_seconds')
        finished = sum(x['completed'] for x in condition['attempts'])
        lines.append(f"| rho {condition['rho']} | {finished} | {bits:.2f}秒 | {first['max_rss_bytes']/1048576:.2f} MiB | {event:.3f}秒 | {first.get('output_bytes',0)/1024**3:.2f} GiB |" if event is not None else f"| rho {condition['rho']} | {finished} | {bits:.2f}秒 | {first['max_rss_bytes']/1048576:.2f} MiB | 未取得 | 未取得 |")
    lines += ['', '上表は最初の完走観測の生値であり、3測定の中央値ではない。時間目標を超えた条件は正式反復を行わず、今回の実測で時間目標未達を確認した条件として扱う。各完走観測ではプロセス開始から出力確定・終了まで最大RSSを測った。完成出力の宣言値を照合し、生成件数1000000も確認した。', '',
              '時間目標は3条件とも未達、メモリは3件の完走観測すべてで2 GiB以内だった。イベント処理だけの時間は25〜52秒なので、それ以外の準備・集計・並べ替え・出力確定に総時間の大半を費やしている。これは区間の差からの判断であり、各処理の個別プロファイルではない。出力サイズはmanifestを含む保存物の合計である。', '',
              '通常／高負荷は各1000000件すべて成功した。過負荷は成功833375件、破棄166241件、未完了384件で合計1000000件。規定Tでの終了による未完了も件数保存へ含め、要求の全件成功とCLIの完走を区別する。', '',
              '## 測定条件と判定範囲', '',
              '32コントローラ、各31250要求、500 kbit/s、標準ID 0x100〜0x11F、8 byte全0、容量64、処理／経路遅延0、metrics-window=1msを使用した。入力とソース、Cargo.lock、Rust 1.85.0 releaseバイナリを固定し、同時実行は1件、観測したシミュレータのthread数も記録した。GNU timeは準備から出力確定までのwallと最大RSSを測定し、出力照合はCLI時間に含めない。CPUプロファイリングやビルドを重ねていない。', '',
              '今回の方針は各条件の完走観測を1回行い、120秒以内に収まる条件だけをウォームアップ＋3測定へ進めるものである。観測で時間目標を超えた条件について正式な中央値・3反復の再現性合格を主張しない。native基準機の正式合否も未検証である。2 GiB以内という記述は実際に完走した観測の全プロセスRSSについてであり、3測定の最大値による正式判定とは区別する。', '',
              '## 出力・ソースと証跡', '',
              '各実行のmanifestの4ファイルについてbyte数とSHA-256を実ファイルと照合し、simulation JSONとrun_idを正規化したCSVのhashを記録した。通常／高負荷／過負荷は別の入力なので、条件間の出力同一性は要求しない。同条件を1回しか完走させていない場合は反復再現性の確認として扱わない。', '',
              f"測定バイナリSHA-256は`{before['binary']['sha256']}`。先の[統合版の検証記録](can-output-integrated-2026-10-08.md)で543テスト・fmt・Clippy・release buildを通過した版と、開始前後のソース184ファイル・バイナリ・構築入力のhashが一致することを確認した。製品コードを今回変更していない。", '',
              '[機械可読記録](' + STEM + '.json)と同名supportディレクトリへ入力・生成式、source hash mapと実ソース、gateログ、GNU time、stdout／stderr、RSSサンプル、manifest、metadata、照合スクリプトを保存した。大きな結果ファイルはhashとmanifest照合後に今回生成したものだけを削除した。実行バイナリはローカル`/tmp/dir-million-integrated-2026-10-08/bin/`へ保持し、リポジトリに収録していない。`support_relative_path`は`inventory_base_directory`から解決する。', '',
              '[独立照合結果](' + STEM + '/independent-verification.json)に、生GNU timeと全記録、生成件数、manifest宣言、source／binary／input pinの検証を記録した。', '',
              '以前の180秒中断記録や小規模の短縮率から今回の完了時間を推定せず、実測した完走値で比較する。旧記録は当時の証跡として保持する。', '']
    md_path.write_text('\n'.join(lines))
    print(json.dumps({'report': str(report_path), 'markdown': str(md_path), 'support_files': len(inventory)}))


if __name__ == '__main__':
    main()
