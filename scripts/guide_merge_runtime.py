"""Trusted-main collector and statically-disabled scoped writer. No PR code execution."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urlsplit, parse_qsl
from urllib.request import Request, build_opener, HTTPRedirectHandler
from guide_scope_policy import ALLOWLIST, safe_content, allowed_path, SHA, Stop, need
from guide_scope_policy import GENERATED_REPORTS
from guide_merge_generated import verify_generated

POLICY_PATH = '.github/guide-merge-policy.json'


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args):
        raise Stop('api_redirect_refused')


class GitHub:
    def __init__(self, repository, token='', write_enabled=False):
        need(repository == 'hideki-ozu/DIR-Simulator', 'repository_mismatch')
        self.repository = repository
        self.prefix = '/repos/' + repository
        self.token = token
        self.write_enabled = write_enabled

    def request(self, method, route, body=None):
        need(route.startswith(self.prefix + '/') and '\n' not in route, 'api_route_scope')
        if method != 'GET':
            need(self.write_enabled is True and self.token, 'transport_writes_disabled')
            merge = method == 'PUT' and re.fullmatch(re.escape(self.prefix) + r'/pulls/[0-9]+/merge', route)
            dispatch = method == 'POST' and route == self.prefix + '/actions/workflows/guide-pages.yml/dispatches'
            need(merge or dispatch, 'mutation_endpoint_forbidden')
            if merge:
                need(isinstance(body, dict) and set(body) == {'sha', 'merge_method'} and
                     SHA.fullmatch(body['sha']) and body['merge_method'] == 'merge', 'merge_body_forbidden')
            else:
                need(isinstance(body, dict) and set(body) == {'ref', 'inputs'} and body['ref'] == 'main' and
                     set(body['inputs']) == {'expected_sha'} and SHA.fullmatch(body['inputs']['expected_sha']), 'dispatch_body_forbidden')
        headers = {'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2026-03-10', 'User-Agent': 'dir-guide-scoped-review'}
        if self.token:
            headers['Authorization'] = 'Bearer ' + self.token
        data = None if body is None else json.dumps(body).encode()
        req = Request('https://api.github.com' + route, data=data, headers=headers, method=method)
        try:
            with build_opener(NoRedirect()).open(req, timeout=30) as r:
                raw = r.read(16_000_001)
                need(len(raw) <= 16_000_000, 'api_response_too_large')
                return (json.loads(raw) if raw else None), r.headers.get('Link', '')
        except HTTPError as e:
            raise Stop('api_http_' + str(e.code)) from None
        except (URLError, TimeoutError, ValueError):
            # Never automatically retry uncertain merge or dispatch writes.
            raise Stop('api_unavailable_or_ambiguous_write') from None

    def get(self, route):
        return self.request('GET', self.prefix + route)[0]

    def pages(self, route, key=None, cap=10000):
        route = self.prefix + route
        rows, expected, seen = [], None, set()
        for _ in range(100):
            need(route not in seen, 'pagination_loop')
            seen.add(route)
            data, link = self.request('GET', route)
            page = data if key is None else data[key]
            need(isinstance(page, list), 'pagination_schema')
            rows.extend(page)
            need(len(rows) < cap, 'pagination_cap')
            if key and 'total_count' in data:
                if expected is None:
                    expected = data['total_count']
                need(expected == data['total_count'] and expected < cap, 'pagination_total_changed_or_capped')
            matches = re.findall(r'<([^>]+)>;\s*rel="next"', link)
            need(len(matches) <= 1, 'pagination_ambiguous')
            if not matches:
                need(expected is not None or len(page) < 100, 'pagination_full_page_ambiguous')
                need(expected is None or len(rows) == expected, 'pagination_count_mismatch')
                return rows
            parsed = urlsplit(matches[0])
            need(parsed.scheme == 'https' and parsed.netloc == 'api.github.com' and
                 parsed.path == urlsplit('https://api.github.com' + route).path and not parsed.fragment,
                 'pagination_foreign_link')
            route = parsed.path + '?' + parsed.query
        raise Stop('pagination_unbounded')


def blob_digest(data):
    return hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()


class PublicDeployments(GitHub):
    """Credential-free GETs for public deployment evidence only; no fallback."""
    def __init__(self, repository):
        super().__init__(repository, token='', write_enabled=False)

    def request(self, method, route, body=None):
        need(method == 'GET' and body is None and not self.token and not self.write_enabled,
             'public_deployment_transport_read_only')
        parsed = urlsplit(route)
        listing = parsed.path == self.prefix + '/deployments'
        statuses = re.fullmatch(re.escape(self.prefix) + r'/deployments/[1-9][0-9]*/statuses', parsed.path)
        need(not parsed.scheme and not parsed.netloc and not parsed.fragment and (listing or statuses),
             'public_deployment_route_forbidden')
        try:
            pairs = parse_qsl(parsed.query, strict_parsing=True)
        except ValueError:
            raise Stop('public_deployment_query_forbidden') from None
        params = dict(pairs)
        required = {'sha', 'environment', 'per_page'} if listing else {'per_page'}
        need(len(params) == len(pairs) and params.get('per_page') == '100' and
             required <= set(params) <= required | {'page'}, 'public_deployment_query_forbidden')
        if listing:
            need(SHA.fullmatch(params['sha']) and params['environment'] == 'github-pages',
                 'public_deployment_scope_forbidden')
        if 'page' in params:
            need(re.fullmatch(r'[1-9][0-9]*', params['page']) and int(params['page']) <= 100,
                 'public_deployment_page_forbidden')
        # No authenticated attempt or alternate credential is tried on failure.
        return super().request(method, route, body)


def deployment_rows(api, sha):
    rows = api.pages('/deployments?' + urlencode({'sha': sha, 'environment': 'github-pages', 'per_page': 100}))
    need(rows and all(type(d['id']) is int and d['id'] > 0 and d['sha'] == sha and
                      d['environment'] == 'github-pages' for d in rows), 'deployment_missing_or_wrong_scope')
    need(len({d['id'] for d in rows}) == len(rows), 'deployment_duplicate_ids')
    return rows


def deployment_preflight(api, sha):
    """Prove both public read endpoints before a merge can be attempted."""
    rows = deployment_rows(api, sha)
    deployment = max(rows, key=lambda d: d['id'])
    statuses = api.pages('/deployments/' + str(deployment['id']) + '/statuses?per_page=100')
    need(statuses and statuses[0]['state'] == 'success', 'public_deployment_preflight_not_success')
    return {'source': 'github_public_rest_no_credentials', 'base_sha': sha,
            'deployment_id': deployment['id']}


class Collector:
    def __init__(self, api, policy, trusted_sha):
        self.api, self.policy, self.trusted_sha = api, policy, trusted_sha
        need(SHA.fullmatch(trusted_sha), 'trusted_sha_invalid')

    def blob(self, sha, limit=2_000_000):
        need(SHA.fullmatch(sha), 'blob_sha_invalid')
        obj = self.api.get('/git/blobs/' + sha)
        need(obj['encoding'] == 'base64' and obj['sha'] == sha and obj['size'] <= limit, 'blob_schema_or_size')
        data = base64.b64decode(obj['content'].replace('\n', ''), validate=True)
        need(len(data) == obj['size'] and blob_digest(data) == sha, 'blob_digest_mismatch')
        return data

    def tree(self, commit_sha):
        commit = self.api.get('/git/commits/' + commit_sha)
        need(commit['sha'] == commit_sha and SHA.fullmatch(commit['tree']['sha']), 'commit_tree_mismatch')
        obj = self.api.get('/git/trees/' + commit['tree']['sha'] + '?recursive=1')
        need(obj['sha'] == commit['tree']['sha'] and obj['truncated'] is False, 'tree_truncated')
        tree = {}
        for row in obj['tree']:
            need(row['path'] not in tree, 'duplicate_tree_path')
            tree[row['path']] = row
        return tree

    def pr(self, number):
        p = self.api.get('/pulls/' + str(number))
        cfg = self.policy
        need(type(number) is int and number > cfg['after_pr'] and p['number'] == number, 'preactivation_pr_excluded')
        need(p['state'] == 'open' and p['draft'] is False, 'draft_or_closed')
        need(p['user']['id'] == cfg['actor_id'] and p['user']['login'] == cfg['actor_login'], 'actor_not_allowed')
        need(p['head']['repo']['full_name'] == cfg['repository'] and p['base']['repo']['full_name'] == cfg['repository'] and
             p['base']['ref'] == 'main', 'fork_or_base_scope')
        if cfg['enabled'] is True:
            record = cfg['activation_record']
            need(isinstance(record, dict) and record['actor_id'] == cfg['actor_id'] and
                 record['after_pr'] == cfg['after_pr'] and re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z', record['approved_at']) and
                 p['created_at'] > record['approved_at'], 'preactivation_or_unknown_approval_record')
        need(SHA.fullmatch(p['head']['sha']) and SHA.fullmatch(p['base']['sha']), 'pr_sha_invalid')
        return p

    def rules(self):
        rules = self.api.pages('/rules/branches/main?per_page=100')
        need(rules, 'active_main_rules_missing')
        types = {r['type'] for r in rules}
        need({'deletion', 'non_fast_forward', 'required_status_checks'} <= types and
             types <= {'deletion', 'non_fast_forward', 'required_status_checks', 'pull_request'}, 'rules_missing_or_unknown')
        # Branch protection governs all PRs; scoped eligibility is checked by
        # the writer directly and must never block ordinary human merges.
        required = set(self.policy['branch_required_checks'])
        known = self.policy['required_checks']
        scoped = {name for name, path in known.items() if path == self.policy['validation_workflow']}
        need(required and required <= set(known) and not required & scoped, 'branch_check_policy_invalid')
        found = set()
        for rule in rules:
            need(rule['ruleset_source_type'] == 'Repository' and rule['ruleset_source'] == self.policy['repository'],
                 'parent_rules_require_separate_review')
            if rule['type'] == 'required_status_checks':
                params = rule['parameters']
                need(params['strict_required_status_checks_policy'] is True, 'strict_checks_disabled')
                pairs = {(c['context'], c['integration_id']) for c in params['required_status_checks']}
                need(not {name for name, _ in pairs} & scoped, 'scoped_check_blocks_general_prs')
                need({(name, self.policy['github_actions_app_id']) for name in required} <= pairs,
                     'required_checks_or_provider_mismatch')
                # Additional protections are never removed to fit this policy.
                # Stop until their check provenance is explicitly reviewed and
                # included in the trusted workflow mapping.
                need(pairs <= {(name, self.policy['github_actions_app_id']) for name in known},
                     'additional_required_checks_need_review')
                found |= {c[0] for c in pairs}
        need(required <= found, 'required_checks_missing')
        for ident in {r['ruleset_id'] for r in rules}:
            need(type(ident) is int and ident > 0, 'ruleset_identity_mismatch')
            ruleset = self.api.get('/rulesets/' + str(ident) + '?includes_parents=true')
            need(type(ruleset['id']) is int and ruleset['id'] == ident and ruleset['source_type'] == 'Repository' and
                 ruleset['source'] == self.policy['repository'], 'ruleset_identity_mismatch')
            need(ruleset['enforcement'] == 'active' and ruleset['target'] == 'branch', 'ruleset_inactive')
            # GitHub hides bypass_actors from callers without ruleset write access.
            # Missing is NOT equivalent to empty; no additional credential is created.
            need('bypass_actors' in ruleset, 'bypass_evidence_not_visible')
            need(isinstance(ruleset['bypass_actors'], list), 'bypass_evidence_malformed')
            need(ruleset['bypass_actors'] == [], 'bypass_actors_present')
            refs = ruleset['conditions']['ref_name']
            need('refs/heads/main' in refs['include'] and refs['exclude'] == [], 'ruleset_scope_unknown')
        return rules

    def files(self, p, base_tree, head_tree):
        need(0 < p['changed_files'] < 3000, 'file_count_out_of_bounds')
        rows = self.api.pages('/pulls/' + str(p['number']) + '/files?per_page=100', cap=3000)
        need(len(rows) == p['changed_files'], 'file_count_mismatch')
        expected, seen, files = set(), set(), {}
        for row in rows:
            path = row['filename']; allowed_path(path, generated=True)
            need(path not in seen, 'duplicate_file')
            seen.add(path); expected.add(path)
            status = row['status']
            if path in GENERATED_REPORTS:
                need(status == 'modified', 'generated_report_must_be_modified')
            need(status in {'added', 'modified', 'renamed'}, 'deletion_or_unknown_status')
            old = row.get('previous_filename') if status == 'renamed' else path
            if status == 'renamed':
                allowed_path(old); need(old != path and old not in head_tree, 'rename_old_path_retained')
                need(old not in seen, 'duplicate_rename_old_path')
                seen.add(old)
                expected.add(old)
            else:
                need(row.get('previous_filename') is None, 'unexpected_rename_metadata')
            h = head_tree[path]
            need(h['type'] == 'blob' and h['mode'] == '100644' and row['sha'] == h['sha'], 'head_mode_type_sha')
            if status == 'added':
                need(path not in base_tree, 'added_path_exists')
            else:
                b = base_tree[old]
                need(b['type'] == 'blob' and b['mode'] == '100644', 'base_mode_type')
            data = self.blob(h['sha'], 4_000_000 if path in GENERATED_REPORTS else 2_000_000)
            safe_content(path, base64.b64encode(data).decode(), h['sha'], generated=True)
            files[path] = data
        changed = {path for path in set(base_tree) | set(head_tree)
                   if base_tree.get(path) != head_tree.get(path) and
                   (base_tree.get(path, {}).get('type') != 'tree' or head_tree.get(path, {}).get('type') != 'tree')}
        need(changed == expected, 'tree_diff_file_list_mismatch')
        if set(files) & GENERATED_REPORTS:
            verify_generated(Path(__file__).resolve().parents[1], files, trusted_sha=self.trusted_sha)
        return files

    def checks(self, p, base_tree, head_tree):
        results = []
        for name, path in self.policy['required_checks'].items():
            need(base_tree[path]['sha'] == head_tree[path]['sha'] and base_tree[path]['mode'] == '100644', 'workflow_changed')
            workflow = self.api.get('/actions/workflows/' + path.rsplit('/', 1)[1])
            need(workflow['path'] == path and workflow['state'] == 'active', 'workflow_not_trusted')
            query = urlencode({'head_sha': p['head']['sha'], 'event': 'pull_request', 'per_page': 100})
            runs = self.api.pages('/actions/workflows/' + str(workflow['id']) + '/runs?' + query, key='workflow_runs', cap=1000)
            matches = [r for r in runs if any(x['number'] == p['number'] for x in r['pull_requests'])]
            need(matches, 'required_run_missing')
            # A later failed/pending run must invalidate an older green run.
            run = max(matches, key=lambda r: (r['run_number'], r['run_attempt'], r['id']))
            need(run['repository']['full_name'] == self.policy['repository'] and run['head_repository']['full_name'] == self.policy['repository'] and
                 run['workflow_id'] == workflow['id'] and run['path'] == path and run['event'] == 'pull_request' and
                 run['head_sha'] == p['head']['sha'] and run['status'] == 'completed' and run['conclusion'] == 'success', 'run_wrong_or_not_success')
            jobs = self.api.pages('/actions/runs/' + str(run['id']) + '/attempts/' + str(run['run_attempt']) + '/jobs?per_page=100', key='jobs')
            jobs = [j for j in jobs if j['name'] == name]
            need(len(jobs) == 1, 'check_name_missing_or_ambiguous')
            job = jobs[0]
            need(job['status'] == 'completed' and job['conclusion'] == 'success' and
                 job['run_id'] == run['id'] and job['run_attempt'] == run['run_attempt'], 'job_failed_skipped_or_stale')
            check_url = urlsplit(job['check_run_url'])
            need(check_url.scheme == 'https' and check_url.netloc == 'api.github.com' and
                 re.fullmatch(re.escape(self.api.prefix) + r'/check-runs/[0-9]+', check_url.path) and
                 not check_url.query and not check_url.fragment, 'check_url_untrusted')
            check = self.api.get(check_url.path[len(self.api.prefix):])
            need(check['name'] == name and check['app']['id'] == self.policy['github_actions_app_id'] and
                 check['app']['slug'] == 'github-actions' and check['status'] == 'completed' and check['conclusion'] == 'success', 'check_spoofed')
            need(check['check_suite']['id'] == run['check_suite_id'] and check['head_sha'] == p['head']['sha'], 'check_wrong_head_or_suite')
            results.append({'name': name, 'run_id': run['id'], 'run_attempt': run['run_attempt']})
        return results

    def collect(self, number, require_checks=True):
        try:
            return self._collect(number, require_checks)
        except (KeyError, TypeError, ValueError, AttributeError):
            raise Stop('missing_or_malformed_api_evidence') from None

    def _collect(self, number, require_checks):
        main = self.api.get('/git/ref/heads/main')['object']['sha']
        need(main == self.trusted_sha, 'main_moved_from_trusted_code')
        p = self.pr(number)
        need(p['base']['sha'] == main, 'base_moved')
        comparison = self.api.get('/compare/' + main + '...' + p['head']['sha'])
        need(comparison['merge_base_commit']['sha'] == main and comparison['status'] == 'ahead', 'head_not_current_base_descendant')
        rules = self.rules() if require_checks else []
        base_tree, head_tree = self.tree(main), self.tree(p['head']['sha'])
        need(POLICY_PATH in base_tree and self.blob(base_tree[POLICY_PATH]['sha']) == self.policy['_bytes'], 'policy_not_trusted_main')
        files = self.files(p, base_tree, head_tree)
        checks = self.checks(p, base_tree, head_tree) if require_checks else []
        fresh = self.pr(number)
        need(fresh['head']['sha'] == p['head']['sha'] and fresh['base']['sha'] == main and
             self.api.get('/git/ref/heads/main')['object']['sha'] == main, 'head_or_base_race')
        if require_checks:
            need(fresh['mergeable'] is True and fresh['mergeable_state'] == 'clean', 'conflict_or_unknown_merge_state')
        return {'number': number, 'head_sha': p['head']['sha'], 'base_sha': main,
                'files': files, 'checks': checks, 'rules': rules}


def load_policy(path):
    raw = Path(path).read_bytes()
    policy = json.loads(raw)
    need(policy['schema_version'] == 1 and policy['repository'] == 'hideki-ozu/DIR-Simulator', 'policy_schema')
    policy['_bytes'] = raw
    return policy


class Writer:
    def __init__(self, collector, env=None, publication_api=None):
        self.collector = collector
        self.api = collector.api
        self.publication_api = publication_api if publication_api is not None else PublicDeployments(self.api.repository)
        self.env = os.environ if env is None else env
        self.audit = {'merge_state': 'not_attempted', 'dispatch_state': 'not_attempted'}

    def execute(self, number, dry_run=True, trigger_run=None, poll=lambda: time.sleep(10), attempts=60):
        p = self.collector.policy
        if not dry_run:
            need(p['enabled'] is True and p['activation_record'] and self.env.get('GUIDE_SCOPED_MERGE_ENABLED') == 'enabled', 'activation_disabled')
            need(self.env.get('GITHUB_EVENT_NAME') == 'workflow_run' and self.env.get('GITHUB_REF') == 'refs/heads/main' and
                 self.env.get('GITHUB_REPOSITORY') == p['repository'] and
                 self.env.get('GITHUB_WORKFLOW_REF') == p['repository'] + '/' + p['writer_workflow'] + '@refs/heads/main' and
                 self.env.get('GITHUB_SHA') == self.collector.trusted_sha, 'writer_not_trusted_main_context')
        first = self.collector.collect(number)
        if dry_run:
            return {'dry_run': True, 'merge_authorized': False, 'head_sha': first['head_sha'], 'files': sorted(first['files'])}
        need(type(trigger_run) is int and trigger_run > 0, 'trigger_run_missing')
        trigger = self.api.get('/actions/runs/' + str(trigger_run))
        trigger_names = {name for name, path in p['required_checks'].items()
                         if path == trigger['path'] and path in {p['validation_workflow'], p['pages_workflow']}}
        need(trigger_names and trigger['event'] == 'pull_request' and
             trigger['head_sha'] == first['head_sha'] and trigger['status'] == 'completed' and
             trigger['conclusion'] == 'success' and len(trigger['pull_requests']) == 1 and
             trigger['pull_requests'][0]['number'] == number and
             any(c['name'] in trigger_names and c['run_id'] == trigger_run and
                 c['run_attempt'] == trigger['run_attempt'] for c in first['checks']),
             'trigger_run_ambiguous_or_stale')
        self.audit['public_deployment_preflight'] = deployment_preflight(self.publication_api, first['base_sha'])
        # Repeat every read and every rule/file/check before the atomic SHA guard.
        last = self.collector.collect(number)
        need(last['head_sha'] == first['head_sha'] and last['base_sha'] == first['base_sha'] and
             last['checks'] == first['checks'] and last['files'] == first['files'] and last['rules'] == first['rules'], 'prewrite_snapshot_changed')
        self.audit['merge_state'] = 'attempted_unconfirmed'
        merged, _ = self.api.request('PUT', self.api.prefix + '/pulls/' + str(number) + '/merge',
                                     {'sha': last['head_sha'], 'merge_method': 'merge'})
        need(merged and merged['merged'] is True and SHA.fullmatch(merged['sha']), 'merge_not_confirmed')
        merge_sha = merged['sha']
        self.audit.update(merge_state='confirmed', merge_sha=merge_sha)
        pr = self.api.get('/pulls/' + str(number))
        need(pr['merged'] is True and pr['merge_commit_sha'] == merge_sha, 'merge_response_not_reconciled')
        need(self.api.get('/git/ref/heads/main')['object']['sha'] == merge_sha, 'main_moved_before_dispatch')
        self.audit['dispatch_state'] = 'attempted_unconfirmed'
        dispatched, _ = self.api.request('POST', self.api.prefix + '/actions/workflows/guide-pages.yml/dispatches',
                                         {'ref': 'main', 'inputs': {'expected_sha': merge_sha}})
        # Current API returns the created run ID; older 204 responses stop as
        # ambiguous rather than guessing a concurrent run or repeating dispatch.
        need(dispatched and type(dispatched['workflow_run_id']) is int, 'dispatch_run_unconfirmed')
        run_id = dispatched['workflow_run_id']
        self.audit.update(dispatch_state='run_confirmed', publication_run_id=run_id)
        for _ in range(attempts):
            run = self.api.get('/actions/runs/' + str(run_id))
            need(run['head_sha'] == merge_sha and run['event'] == 'workflow_dispatch' and
                 run['path'] == p['pages_workflow'] and run['repository']['full_name'] == p['repository'], 'dispatch_run_mismatch')
            if run['status'] == 'completed':
                need(run['conclusion'] == 'success', 'publication_failed')
                jobs = self.api.pages('/actions/runs/' + str(run_id) + '/attempts/' + str(run['run_attempt']) + '/jobs?per_page=100', key='jobs')
                needed = {'MkDocs strict build and links', 'Publish main to GitHub Pages'}
                relevant = [j for j in jobs if j['name'] in needed]
                need(len(relevant) == 2 and {j['name'] for j in relevant} == needed and
                     all(j['conclusion'] == 'success' and j['status'] == 'completed' and
                         j['run_id'] == run_id and j['run_attempt'] == run['run_attempt'] for j in relevant), 'publication_jobs_not_success')
                matching = deployment_rows(self.publication_api, merge_sha)
                statuses = self.publication_api.pages('/deployments/' + str(max(matching, key=lambda d: d['id'])['id']) + '/statuses?per_page=100')
                need(statuses and statuses[0]['state'] == 'success' and statuses[0]['log_url'].startswith(
                    'https://github.com/' + p['repository'] + '/actions/runs/' + str(run_id) + '/'), 'deployment_not_success_for_run')
                need(self.api.get('/git/ref/heads/main')['object']['sha'] == merge_sha, 'main_moved_after_publication')
                return {'merged': True, 'merge_sha': merge_sha, 'publication': 'success', 'run_id': run_id,
                        'public_url': statuses[0]['environment_url']}
            poll()
        raise Stop('publication_timeout_no_success_claim')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pr', type=int, required=True)
    parser.add_argument('--trusted-sha', required=True)
    parser.add_argument('--policy', default=POLICY_PATH)
    parser.add_argument('--write', action='store_true')
    parser.add_argument('--trigger-run', type=int)
    args = parser.parse_args()
    try:
        policy = load_policy(args.policy)
        api = GitHub(policy['repository'], os.environ.get('GH_TOKEN', ''), write_enabled=args.write)
        writer = Writer(Collector(api, policy, args.trusted_sha))
        result = writer.execute(args.pr, dry_run=not args.write, trigger_run=args.trigger_run)
        print(json.dumps(result, ensure_ascii=False)); return 0
    except (Stop, KeyError, TypeError, ValueError, AttributeError, OSError) as e:
        print(json.dumps({'stopped': True, 'merge_authorized': False,
                          'reason': str(e) if isinstance(e, Stop) else 'missing_or_malformed_api_evidence',
                          **(writer.audit if 'writer' in locals() else {'merge_state': 'not_attempted', 'dispatch_state': 'not_attempted'})}))
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
