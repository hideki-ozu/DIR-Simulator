#!/usr/bin/env python3
"""Check the scoped owner-statement receipt, not source authorship or rights."""
import json
from pathlib import Path


def check(root):
    packet = root / 'docs/verification/results/ac0008-origin-followup-2026-10-10'
    statement = json.loads((packet / 'owner-origin-statement.json').read_text(encoding='utf-8'))
    old = json.loads((root / 'docs/verification/results/ac0008-review-2026-10-10/ledger-review-proposals.json').read_text(encoding='utf-8'))
    selection = json.loads((packet / 'license-selection-2026-10-10.json').read_text(encoding='utf-8'))
    expected = 'このプロジェクトの生成物は、すべてAI作成、もしくはAIが選択して入手した別プロジェクトの成果物です。'
    assert statement['statement_verbatim'] == expected
    assert statement['received_at'] == '2026-10-10T13:40:00Z'
    assert statement['source_message_id'] == 'Sentinel_abde86d1dc2c8191a278971e16b05f09'
    assert statement['current_status'] == 'owner_project_wide_explanation_received'
    for flag in ['human_independent_creation_claim', 'all_original_claim', 'all_files_attributed', 'all_models_tools_identified', 'all_external_rights_confirmed', 'license_choice_changed', 'historical_unanswered_records_modified']:
        assert statement[flag] is False, flag
    rows = statement['components']
    assert len(rows) == 50
    assert [x['key'] for x in rows] == [x['key'] for x in old['rows']]
    assert len({x['key'] for x in rows}) == 50
    for row, historical in zip(rows, old['rows']):
        cells = historical['ledger_cells']
        assert row['confirmed_external_reference'] == cells[3]
        assert row['saved_license_and_notice_reference'] == cells[9 if historical['kind'] == 'Cargo' else 6]
        assert row['original_expression'] == historical['route_options']
        assert row['individual_AI_creation_or_selection'] == 'unknown'
        assert row['model'] is None and row['tool'] is None
        assert row['individual_source_file_mapping'] == 'unknown'
        assert row['complete_rights_conditions'] == 'not established'
    assert statement['license_choice_record'] == 'license-selection-2026-10-10.json'
    assert selection['approval']['date'] == '2026-10-10T13:08:40Z'
    assert sum(x['choice_approved'] for x in selection['rows']) == 40
    for path, phrase in [('README.md', '本人の全体説明'), ('public-history-notice-review.md', '全体説明は受領済み')]:
        assert phrase in (packet / path).read_text(encoding='utf-8')
    assert not any(row['approval'] != 'unapproved' for row in old['rows'])
    print('PASS: project-wide owner explanation received; 50 fixed component references; 40 MIT choices kept separate.')
    print('SCOPE: receipt consistency only; no file-level authorship, model/tool attribution or complete rights clearance.')


if __name__ == '__main__':
    check(Path(__file__).resolve().parents[2])
