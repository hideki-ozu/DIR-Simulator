"""Offline, fail-closed review prototype. No GitHub client or mutation capability.

Input is a synthetic snapshot, NOT trusted evidence or a merge authorization.
The future trusted-main collector/writer is intentionally absent from this PR.
"""
import base64
import hashlib
import html
import json
from pathlib import PurePosixPath
import re
import sys
from urllib.parse import unquote

REPOSITORY = 'hideki-ozu/DIR-Simulator'
GUIDES = {'docs/guide/index.md', 'docs/guide/初めてのCAN実行.md',
          'docs/guide/Viewerで結果を読む.md', 'docs/guide/CANの調停.md',
          'docs/guide/設定・用語・FAQ.md'}
IMAGES = {'docs/guide/assets/guide-minimal-viewer.png', 'docs/guide/assets/guide-fast-viewer.png',
          'docs/guide/assets/guide-id-swap-viewer.png', 'docs/guide/assets/guide-viewer-at-238us.png'}
SAMPLES = {'examples/guide/can/minimal.ini', 'examples/guide/can/fast.ini',
           'examples/guide/can/id-swap.ini', 'examples/guide/can/minimal.json',
           'examples/guide/can/id-swap.json'}
ALLOWLIST = GUIDES | IMAGES | SAMPLES
GENERATED_REPORTS = {'docs/要件トレーサビリティ一覧.md', 'docs/要件トレーサビリティ一覧.html'}
# Derived reports are not arbitrary editable guide input. Every caller accepting
# them must additionally prove exact regeneration with trusted-base code.
COLLECTED_PATHS = ALLOWLIST | GENERATED_REPORTS
REQUIRED_CHECKS = {'MkDocs strict build and links', 'Guide sample reproduction'}
WORKFLOW_PATH = '.github/workflows/guide-pages.yml'
SHA = re.compile(r'^[0-9a-f]{40}$')


class Stop(Exception):
    pass


def need(condition, reason):
    if not condition:
        raise Stop(reason)


def allowed_path(path, generated=False):
    need(isinstance(path, str), 'path_missing')
    need(path == str(PurePosixPath(path)) and '\\' not in path and
         not any(p in {'.', '..', ''} for p in path.split('/')), 'noncanonical_path')
    need(path in (COLLECTED_PATHS if generated else ALLOWLIST), 'path_outside_allowlist')


def safe_content(path, contents, blob_sha, generated=False):
    need(isinstance(contents, str), 'content_missing')
    data = base64.b64decode(contents, validate=True)
    need(hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest() == blob_sha, 'blob_digest_mismatch')
    limit = 4_000_000 if path in GENERATED_REPORTS else (2_000_000 if path in IMAGES else 200_000)
    need(len(data) <= limit, 'blob_too_large')
    if path in GENERATED_REPORTS:
        need(generated is True, 'generated_report_requires_trusted_regeneration')
        data.decode('utf-8')
        need(b'\x00' not in data, 'binary_text')
        return
    if path in IMAGES:
        # Only a conservative signature screen; future trusted CI must fully decode
        # and re-encode PNG and reject malformed/polyglot images before eligibility.
        need(data.startswith(b'\x89PNG\r\n\x1a\n'), 'image_not_png')
        return
    text = data.decode('utf-8')
    need('\x00' not in text, 'binary_text')
    normalized = text
    for _ in range(4):
        normalized = html.unescape(unquote(normalized))
    need(not re.search(r'<\s*[!/?A-Za-z]|(?:javascript|vbscript|data|file)\s*:', normalized, re.I), 'active_or_local_content')
    need(not re.search(r'(?:javascript|vbscript|data|file):', re.sub(r'\s', '', normalized), re.I), 'spaced_active_uri')
    need(not re.search(r'!\[[^\]]*\]\(\s*(?:https?:|//)', normalized, re.I), 'external_image_embed')
    if path.endswith('.json'):
        value = json.loads(text)
        need(isinstance(value, dict) and value.get('schema_version') == 1 and
             isinstance(value.get('generators'), list), 'sample_schema_unknown')
        need(set(value) == {'schema_version', 'generators'} and len(value['generators']) <= 3, 'sample_unbounded_or_extra_fields')
        total = 0
        for generator in value['generators']:
            need(set(generator) == {'id', 'kind', 'node', 'start', 'period', 'count', 'frame'} and
                 generator['kind'] == 'can.periodic.v1' and generator['node'] in {'Main.a', 'Main.b', 'Main.c'} and
                 type(generator['count']) is int and 0 <= generator['count'] <= 1000, 'sample_generator_out_of_scope')
            frame = generator['frame']
            need(set(frame) == {'format', 'id', 'data'} and frame['format'] in {'standard', 'extended'} and
                 type(frame['id']) is int and isinstance(frame['data'], str) and
                 len(frame['data']) <= 16 and re.fullmatch(r'(?:[0-9a-fA-F]{2})*', frame['data']), 'sample_frame_out_of_scope')
            total += generator['count']
        need(total <= 1000, 'sample_total_unbounded')
    if path.endswith('.ini'):
        # Includes would expand the sample outside the bounded input set.
        need(not re.search(r'^\s*(?:include|extends)\b', text, re.I | re.M), 'sample_include')
        for line in text.splitlines():
            line = line.strip()
            if not line or line.startswith(('#', ';')):
                continue
            if line.startswith('['):
                need(line == '[General]', 'sample_section_unknown')
                continue
            key, value = [p.strip() for p in line.split('=', 1)]
            if key == 'network':
                need(value == 'demo.Main', 'sample_network_changed')
            elif key == 'ned-path':
                need(value == '"models"', 'sample_model_path_changed')
            elif key == 'workload':
                need(value in {'"minimal.json"', '"id-swap.json"'}, 'sample_workload_path_changed')
            else:
                need(key in {'sim-time-limit', 'metrics-window', 'Main.bus.bitrate'} or
                     re.fullmatch(r'Main\.[abc]\.(queueCapacity|rxFilter|txProcessingDelay|rxProcessingDelay)', key),
                     'sample_setting_unknown')


def evaluate(s):
    """Return a review-only decision; merge_authorized is ALWAYS false."""
    try:
        need(s['schema_version'] == 1, 'snapshot_schema_unknown')
        p = s['pr']
        need(p['repository'] == REPOSITORY and p['head_repository'] == REPOSITORY and
             p['author'] == 'hideki-ozu' and p['base_ref'] == 'main', 'untrusted_pr_scope')
        need(type(p['number']) is int and p['number'] > 26, 'existing_pr_excluded')
        need(p['state'] == 'open' and p['draft'] is False, 'draft_or_closed')
        need(p['mergeable'] is True and p['merge_state'] == 'clean', 'conflict_or_unknown')
        head, base = p['head_sha'], p['base_sha']
        need(SHA.fullmatch(head) and SHA.fullmatch(base), 'sha_invalid')
        need(s['fresh_pr']['head_sha'] == head and s['fresh_pr']['base_sha'] == base and
             s['fresh_pr']['draft'] is False and s['fresh_pr']['state'] == 'open', 'head_or_base_changed')
        protection = s['protection']
        need(protection['enforced'] is True and protection['strict'] is True and
             protection['no_bypass'] is True, 'protection_absent_or_weak')
        need(set(protection['required_checks']) == REQUIRED_CHECKS and
             protection['source'] == 'github-actions' and protection['read_complete'] is True,
             'required_check_policy_unknown')
        listing = s['file_listing']
        files = listing['files']
        need(listing['all_pages_read'] is True and listing['has_next_page'] is False and
             listing['truncated'] is False and 0 < len(files) < 3000 and
             len(files) == p['changed_files'], 'file_listing_incomplete')
        need(listing['head_sha'] == head and listing['base_sha'] == base, 'file_listing_stale')
        need(s['trees']['head_sha'] == head and s['trees']['base_sha'] == base and
             s['trees']['complete'] is True and s['trees']['truncated'] is False, 'tree_incomplete_or_stale')
        seen = set()
        for f in files:
            path = f['path']
            allowed_path(path)
            need(path not in seen, 'duplicate_file')
            seen.add(path)
            status = f['status']
            need(status in {'added', 'modified', 'renamed'}, 'deletion_or_unknown_status')
            need(f['head_type'] == 'blob' and f['head_mode'] == '100644', 'symlink_submodule_or_executable')
            if status != 'added':
                need(f['base_type'] == 'blob' and f['base_mode'] == '100644', 'unsafe_old_mode')
            else:
                need(f['base_type'] is None and f['base_mode'] is None, 'added_file_already_exists')
            if status == 'renamed':
                allowed_path(f['previous_path'])
                need(f['previous_path'] != path, 'rename_same_path')
            else:
                need(f.get('previous_path') is None, 'unexpected_previous_path')
            need(f['content_sha'] == f['head_blob_sha'] and SHA.fullmatch(f['head_blob_sha']), 'blob_unverified')
            safe_content(path, f['content_base64'], f['head_blob_sha'])
        evidence = s['evidence']
        need(evidence['all_pages_read'] is True and evidence['has_next_page'] is False and
             evidence['head_sha'] == head and evidence['base_sha'] == base, 'evidence_incomplete_or_stale')
        need(evidence['trusted_workflow_sha'] == s['trusted_main_sha'] and
             s['trusted_main_sha'] == base and evidence['workflow_path'] == WORKFLOW_PATH and
             evidence['repository'] == REPOSITORY and evidence['event'] == 'pull_request' and
             evidence['pull_number'] == p['number'], 'evidence_wrong_provenance')
        checks = evidence['checks']
        need(len(checks) == len(REQUIRED_CHECKS) and {c['name'] for c in checks} == REQUIRED_CHECKS,
             'missing_or_ambiguous_checks')
        need(all(c['status'] == 'completed' and c['conclusion'] == 'success' and
                 c['run_attempt'] == evidence['run_attempt'] and c['run_id'] == evidence['run_id']
                 for c in checks), 'checks_pending_failed_skipped_or_stale')
        need(all(evidence[k] is True for k in ['samples_reproduced', 'public_asset_scan', 'images_decoded_reencoded',
                                             'no_pr_code_in_privileged_job', 'workflow_matches_trusted_main']),
             'validation_not_proven')
        return {'review_candidate': True, 'merge_authorized': False, 'reason': 'prototype_disabled', 'head_sha': head}
    except Stop as e:
        return {'review_candidate': False, 'merge_authorized': False, 'reason': str(e)}
    except (KeyError, TypeError, ValueError, AttributeError, OverflowError):
        return {'review_candidate': False, 'merge_authorized': False, 'reason': 'missing_or_malformed_evidence'}


def main():
    try:
        contents = sys.stdin.read(4_000_001)
        need(len(contents) <= 4_000_000, 'snapshot_too_large')
        snapshot = json.loads(contents)
    except (ValueError, OSError, Stop):
        print(json.dumps({'review_candidate': False, 'merge_authorized': False, 'reason': 'invalid_json'}))
        return 2
    decision = evaluate(snapshot)
    print(json.dumps(decision, ensure_ascii=False))
    return 0 if decision['review_candidate'] else 2


if __name__ == '__main__':
    raise SystemExit(main())
