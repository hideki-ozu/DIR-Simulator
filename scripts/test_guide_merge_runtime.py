"""No live writes: collector/merge/dispatch boundaries use a strict mock server."""
import base64
import copy
import json
from pathlib import Path
import unittest
from unittest.mock import patch
from guide_merge_runtime import Collector, GitHub, Writer, Stop, POLICY_PATH, blob_digest

BASE, HEAD, MERGED = 'b' * 40, 'a' * 40, 'c' * 40
NUMBER = 1000  # Synthetic PR only; never PR26/27 or another real existing PR.


class FakeGitHub(GitHub):
    def __init__(self, policy):
        super().__init__(policy['repository'], token='mock-only', write_enabled=True)
        self.routes, self.calls, self.writes = {}, [], []
    def request(self, method, route, body=None):
        self.calls.append((method, route, copy.deepcopy(body)))
        if method != 'GET': self.writes.append((method, route, copy.deepcopy(body)))
        key = (method, route)
        if key not in self.routes: raise Stop('mock_unexpected_route')
        response = self.routes[key]
        return copy.deepcopy(response(body) if callable(response) else response)
    def add(self, route, response, link='', method='GET'):
        self.routes[(method, self.prefix + route)] = (response, link)


def fixture(enabled=False):
    path = Path(__file__).resolve().parents[1] / POLICY_PATH
    policy = json.loads(path.read_text(encoding='utf-8'))
    policy['enabled'] = enabled
    policy['activation_record'] = {'actor_id': policy['actor_id'], 'after_pr': 27,
                                   'approved_at': '2026-10-06T15:00:00Z'} if enabled else None
    policy['_bytes'] = json.dumps({k:v for k,v in policy.items() if k != '_bytes'}).encode()
    api = FakeGitHub(policy)
    def blob(data):
        sha = blob_digest(data)
        api.add('/git/blobs/' + sha, {'sha': sha, 'encoding': 'base64', 'size': len(data),
                                     'content': base64.b64encode(data).decode()})
        return sha
    def entry(path, data):
        return {'path': path, 'mode': '100644', 'type': 'blob', 'sha': blob(data)}
    path = 'docs/guide/CANの調停.md'
    base = [entry(path, b'# Old\n'), entry(POLICY_PATH, policy['_bytes'])]
    head = [entry(path, b'# New\n'), entry(POLICY_PATH, policy['_bytes'])]
    for workflow in policy['required_checks'].values():
        row = entry(workflow, b'trusted-workflow'); base.append(row); head.append(copy.deepcopy(row))
    api.add('/git/ref/heads/main', {'object': {'sha': BASE}})
    for commit, tree, rows in [(BASE, 'd'*40, base), (HEAD, 'e'*40, head)]:
        api.add('/git/commits/' + commit, {'sha': commit, 'tree': {'sha': tree}})
        api.add('/git/trees/' + tree + '?recursive=1', {'sha':tree,'truncated':False,'tree':rows})
    pr = {'number':NUMBER,'state':'open','draft':False,'user':{'login':policy['actor_login'],'id':policy['actor_id']},
          'head':{'sha':HEAD,'repo':{'full_name':policy['repository']}},
          'base':{'sha':BASE,'ref':'main','repo':{'full_name':policy['repository']}},
          'mergeable':True,'mergeable_state':'clean','changed_files':1,'created_at':'2026-10-07T00:00:00Z'}
    api.add('/pulls/' + str(NUMBER), pr)
    api.add('/compare/' + BASE + '...' + HEAD, {'merge_base_commit':{'sha':BASE},'status':'ahead'})
    checks = [{'context':name,'integration_id':policy['github_actions_app_id']} for name in policy['branch_required_checks']]
    rules = [{'type':t,'ruleset_source_type':'Repository','ruleset_source':policy['repository'],'ruleset_id':1}
             for t in ['deletion','non_fast_forward','required_status_checks']]
    rules[-1]['parameters'] = {'strict_required_status_checks_policy':True,'required_status_checks':checks}
    api.add('/rules/branches/main?per_page=100', rules)
    api.add('/rulesets/1?includes_parents=true', {'enforcement':'active','target':'branch','bypass_actors':[],
             'conditions':{'ref_name':{'include':['refs/heads/main'],'exclude':[]}}})
    api.add('/pulls/'+str(NUMBER)+'/files?per_page=100', [{'filename':path,'sha':head[0]['sha'],'status':'modified'}])
    for wid, (name, workflow) in enumerate(policy['required_checks'].items(),1):
        rid = wid*10+1; suite = wid*10+2
        api.add('/actions/workflows/'+workflow.rsplit('/',1)[1], {'id':wid,'path':workflow,'state':'active'})
        run = {'id':rid,'workflow_id':wid,'path':workflow,'event':'pull_request','head_sha':HEAD,
               'repository':{'full_name':policy['repository']},'head_repository':{'full_name':policy['repository']},
               'status':'completed','conclusion':'success','run_number':10,'run_attempt':2,
               'pull_requests':[{'number':NUMBER}],'check_suite_id':suite}
        api.add('/actions/workflows/'+str(wid)+'/runs?head_sha='+HEAD+'&event=pull_request&per_page=100',
                {'total_count':1,'workflow_runs':[run]})
        api.add('/actions/runs/'+str(rid),run)
        job = {'name':name,'status':'completed','conclusion':'success','run_id':rid,'run_attempt':2,
               'check_run_url':'https://api.github.com'+api.prefix+'/check-runs/'+str(rid)}
        api.add('/actions/runs/'+str(rid)+'/attempts/2/jobs?per_page=100',{'total_count':1,'jobs':[job]})
        api.add('/check-runs/'+str(rid),{'name':name,'app':{'id':15368,'slug':'github-actions'},
                'head_sha':HEAD,'status':'completed','conclusion':'success','check_suite':{'id':suite}})
    collector = Collector(api, policy, BASE)
    env = {'GUIDE_SCOPED_MERGE_ENABLED':'enabled','GITHUB_EVENT_NAME':'workflow_run','GITHUB_REF':'refs/heads/main',
           'GITHUB_REPOSITORY':policy['repository'],'GITHUB_WORKFLOW_REF':policy['repository']+'/'+policy['writer_workflow']+'@refs/heads/main',
           'GITHUB_SHA':BASE}
    return api, collector, env


def publication_routes(api):
    def merge(body):
        assert body == {'sha':HEAD,'merge_method':'merge'}
        api.add('/git/ref/heads/main', {'object':{'sha':MERGED}})
        api.add('/pulls/'+str(NUMBER), {'merged':True,'merge_commit_sha':MERGED})
        return ({'merged':True,'sha':MERGED},'')
    api.routes[('PUT',api.prefix+'/pulls/'+str(NUMBER)+'/merge')] = merge
    api.add('/actions/workflows/guide-pages.yml/dispatches',{'workflow_run_id':99},method='POST')
    api.add('/actions/runs/99',{'head_sha':MERGED,'event':'workflow_dispatch','path':'.github/workflows/guide-pages.yml',
             'repository':{'full_name':api.repository},'status':'completed','conclusion':'success','run_attempt':1})
    jobs = [{'name':n,'status':'completed','conclusion':'success','run_id':99,'run_attempt':1} for n in ['MkDocs strict build and links','Publish main to GitHub Pages']]
    api.add('/actions/runs/99/attempts/1/jobs?per_page=100',{'total_count':2,'jobs':jobs})
    api.add('/deployments?sha='+MERGED+'&environment=github-pages&per_page=100',[{'id':5,'sha':MERGED,'environment':'github-pages'}])
    api.add('/deployments/5/statuses?per_page=100',[{'state':'success','environment_url':'https://hideki-ozu.github.io/DIR-Simulator/',
             'log_url':'https://github.com/'+api.repository+'/actions/runs/99/job/1'}])


class RuntimeTests(unittest.TestCase):
    def test_collector_fetches_all_pr_file_pages_and_rejects_incomplete_page(self):
        api,c,e=fixture()
        path='docs/guide/index.md'; data=b'# Second page\n'; sha=blob_digest(data)
        api.add('/git/blobs/'+sha,{'sha':sha,'encoding':'base64','size':len(data),'content':base64.b64encode(data).decode()})
        api.routes[('GET',api.prefix+'/git/trees/'+'e'*40+'?recursive=1')][0]['tree'].append(
            {'path':path,'mode':'100644','type':'blob','sha':sha})
        api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0]['changed_files']=2
        route='/pulls/'+str(NUMBER)+'/files?per_page=100'
        rows=api.routes[('GET',api.prefix+route)][0]
        api.add(route,rows,'<https://api.github.com'+api.prefix+route+'&page=2>; rel="next"')
        api.add(route+'&page=2',[{'filename':path,'sha':sha,'status':'added'}])
        self.assertIn(path,c.collect(NUMBER)['files'])
        api.add(route+'&page=2',[])
        with self.assertRaises(Stop): c.collect(NUMBER)
        self.assertEqual(api.writes,[])

    def test_general_pr_protection_independent_of_scoped_eligibility(self):
        for mutation in ['code', 'author', 'preapproval']:
            api,c,e=fixture(True)
            pr=api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0]
            if mutation=='author': pr['user']['id']=0
            if mutation=='preapproval': pr['created_at']='2026-10-01T00:00:00Z'
            if mutation=='code':
                api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER)+'/files?per_page=100')][0][0]['filename']='scripts/code.py'
            with self.subTest(mutation=mutation):
                # General branch gates do not depend on scoped eligibility.
                self.assertTrue(c.rules())
                with self.assertRaises(Stop): c.collect(NUMBER,require_checks=False)
                self.assertEqual(api.writes,[])

    def test_scoped_global_required_and_extra_protection_fail_closed(self):
        for name in ['Guide scoped validation', 'Existing security check']:
            api,c,e=fixture(True)
            rules=api.routes[('GET',api.prefix+'/rules/branches/main?per_page=100')][0]
            rules[-1]['parameters']['required_status_checks'].append({'context':name,'integration_id':15368})
            with self.subTest(name=name),self.assertRaises(Stop): c.rules()
            self.assertEqual(api.writes,[])
            self.assertEqual(len(rules[-1]['parameters']['required_status_checks']),2)

    def test_each_workflow_completion_rechecks_all_latest_checks(self):
        for trigger_id, other_id in [(11,21),(21,11)]:
            api,c,e=fixture(True);publication_routes(api)
            runs=api.routes[('GET',api.prefix+'/actions/workflows/'+str(other_id//10)+'/runs?head_sha='+HEAD+'&event=pull_request&per_page=100')][0]
            runs['workflow_runs'][0]['status']='in_progress';runs['workflow_runs'][0]['conclusion']=None
            with self.subTest(trigger_id=trigger_id):
                with self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=trigger_id)
                self.assertEqual(api.writes,[])
                # Second workflow's completion supplies the missing reevaluation.
                runs['workflow_runs'][0]['status']='completed';runs['workflow_runs'][0]['conclusion']='success'
                self.assertEqual(Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=other_id,poll=lambda:None)['publication'],'success')
                self.assertEqual(len(api.writes),2)

    def test_duplicate_completed_event_cannot_merge_twice(self):
        api,c,e=fixture(True);publication_routes(api)
        Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=11,poll=lambda:None)
        for trigger_id in [11,21]:
            with self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=trigger_id)
        self.assertEqual(len([w for w in api.writes if w[0]=='PUT']),1)

    def test_rerun_attempt_or_new_run_during_prewrite_no_merge(self):
        for mutation in ['attempt','new_run','interrupted']:
            api,c,e=fixture(True)
            original=c.collect; calls=[]
            def collect(number):
                calls.append(number)
                result=original(number)
                if len(calls)==2:
                    if mutation=='interrupted': raise Stop('api_unavailable_or_ambiguous_write')
                    result['checks'][0]['run_attempt' if mutation=='attempt' else 'run_id']+=1
                return result
            c.collect=collect
            with self.subTest(mutation=mutation),self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=11)
            self.assertEqual(api.writes,[])

    def test_old_attempt_trigger_even_after_new_attempt_success_stops(self):
        api,c,e=fixture(True)
        api.routes[('GET',api.prefix+'/actions/runs/11')][0]['run_attempt']=1
        with self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=11)
        self.assertEqual(api.writes,[])

    def test_complete_collector_and_default_dry_run(self):
        api,c,e=fixture(); result=Writer(c,e).execute(NUMBER)
        self.assertTrue(result['dry_run']); self.assertFalse(result['merge_authorized']); self.assertEqual(api.writes,[])

    def test_mock_only_merge_and_exact_dispatch_run(self):
        api,c,e=fixture(True); publication_routes(api)
        r=Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=21,poll=lambda:None)
        self.assertEqual(r['publication'],'success'); self.assertEqual(r['merge_sha'],MERGED)
        self.assertEqual(len(api.writes),2); self.assertEqual(api.writes[1][2],{'ref':'main','inputs':{'expected_sha':MERGED}})

    def test_activation_and_context_fail_before_any_read_or_write(self):
        api,c,e=fixture(False)
        with self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=21)
        self.assertEqual(api.calls,[])
        for key in e:
            api,c,env=fixture(True); env[key]='untrusted'
            with self.subTest(key=key),self.assertRaises(Stop): Writer(c,env).execute(NUMBER,dry_run=False,trigger_run=21)
            self.assertEqual(api.calls,[])

    def test_pr_fork_draft_author_and_old_pr(self):
        cases=[('draft',True),('mergeable',None),('mergeable_state','dirty'),('changed_files',3000)]
        for key,val in cases:
            api,c,e=fixture(); p=api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0];p[key]=val
            with self.subTest(key=key),self.assertRaises(Stop): c.collect(NUMBER)
        api,c,e=fixture();api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0]['head']['repo']['full_name']='fork/repo'
        with self.assertRaises(Stop): c.collect(NUMBER)
        api,c,e=fixture();api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0]['user']['id']=0
        with self.assertRaises(Stop): c.collect(NUMBER)
        api,c,e=fixture(); c.policy['after_pr']=NUMBER
        with self.assertRaises(Stop): c.collect(NUMBER)

    def test_rules_missing_hidden_bypass_strict_and_provider(self):
        for mutation in ['missing','hidden','bypass','strict','provider','parent']:
            api,c,e=fixture();rules=api.routes[('GET',api.prefix+'/rules/branches/main?per_page=100')][0]
            rs=api.routes[('GET',api.prefix+'/rulesets/1?includes_parents=true')][0]
            if mutation=='missing': rules.clear()
            if mutation=='hidden': del rs['bypass_actors']
            if mutation=='bypass': rs['bypass_actors']=[{'actor_id':1}]
            if mutation=='strict': rules[-1]['parameters']['strict_required_status_checks_policy']=False
            if mutation=='provider': rules[-1]['parameters']['required_status_checks'][0]['integration_id']=-1
            if mutation=='parent': rules[0]['ruleset_source_type']='Organization'
            with self.subTest(mutation=mutation),self.assertRaises(Stop): c.collect(NUMBER)

    def test_truncated_tree_mode_deletion_rename_and_listing_omission(self):
        for mutation in ['tree','symlink','executable','submodule','delete','rename','omission','unlisted_change','blob']:
            api,c,e=fixture();tree=api.routes[('GET',api.prefix+'/git/trees/'+'e'*40+'?recursive=1')][0]
            files=api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER)+'/files?per_page=100')][0]
            if mutation=='tree': tree['truncated']=True
            if mutation=='symlink': tree['tree'][0]['mode']='120000'
            if mutation=='executable': tree['tree'][0]['mode']='100755'
            if mutation=='submodule': tree['tree'][0]['type']='commit'
            if mutation=='delete': files[0]['status']='removed'
            if mutation=='rename': files[0].update(status='renamed',previous_filename='scripts/evil.py')
            if mutation=='omission': files.clear()
            if mutation=='unlisted_change': tree['tree'].append({'path':'scripts/evil.py','mode':'100644','type':'blob','sha':'f'*40})
            if mutation=='blob': files[0]['sha']='f'*40
            with self.subTest(mutation=mutation),self.assertRaises(Stop): c.collect(NUMBER)

    def test_head_race_and_additional_commit_no_write(self):
        api,c,e=fixture(True); key=('GET',api.prefix+'/pulls/'+str(NUMBER)); old=copy.deepcopy(api.routes[key]); reads=[]
        def pr(_):
            reads.append(1); result=copy.deepcopy(old)
            if len(reads)>=3: result[0]['head']['sha']='f'*40
            return result
        api.routes[key]=pr
        with self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=21)
        self.assertEqual(api.writes,[])

    def test_both_allowlisted_rename_paths_and_old_symlink(self):
        api,c,e=fixture()
        rows=api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER)+'/files?per_page=100')][0]
        rows[0].update(status='renamed',previous_filename=rows[0]['filename'],filename='docs/guide/index.md')
        api.routes[('GET',api.prefix+'/git/trees/'+'e'*40+'?recursive=1')][0]['tree'][0]['path']='docs/guide/index.md'
        self.assertEqual(sorted(c.collect(NUMBER)['files']),['docs/guide/index.md'])
        api.routes[('GET',api.prefix+'/git/trees/'+'d'*40+'?recursive=1')][0]['tree'][0]['mode']='120000'
        with self.assertRaises(Stop): c.collect(NUMBER)

    def test_checks_spoof_wrong_attempt_later_failed_duplicate_or_missing(self):
        for mutation in ['provider','head','attempt','duplicate','missing','latest_failed','workflow','suite']:
            api,c,e=fixture();check=api.routes[('GET',api.prefix+'/check-runs/21')][0]
            jobs=api.routes[('GET',api.prefix+'/actions/runs/21/attempts/2/jobs?per_page=100')][0]
            runs=api.routes[('GET',api.prefix+'/actions/workflows/2/runs?head_sha='+HEAD+'&event=pull_request&per_page=100')][0]
            if mutation=='provider': check['app']['id']=1
            if mutation=='head': check['head_sha']='f'*40
            if mutation=='attempt': jobs['jobs'][0]['run_attempt']=1
            if mutation=='duplicate': jobs['jobs']*=2;jobs['total_count']=2
            if mutation=='missing': jobs['jobs']=[];jobs['total_count']=0
            if mutation=='latest_failed':
                later=copy.deepcopy(runs['workflow_runs'][0]);later.update(id=22,run_number=11,conclusion='failure')
                runs['workflow_runs'].append(later);runs['total_count']=2
            if mutation=='workflow': runs['workflow_runs'][0]['path']='other.yml'
            if mutation=='suite': check['check_suite']['id']=0
            with self.subTest(mutation=mutation),self.assertRaises(Stop): c.collect(NUMBER)

    def test_ambiguous_trigger_no_write(self):
        api,c,e=fixture(True);api.routes[('GET',api.prefix+'/actions/runs/21')][0]['pull_requests'].append({'number':NUMBER+1})
        with self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=21)
        self.assertEqual(api.writes,[])

    def test_merge_denied_or_uncertain_is_never_retried(self):
        for reason in ['api_http_409', 'api_http_403', 'api_unavailable_or_ambiguous_write']:
            api,c,e=fixture(True)
            def deny(_): raise Stop(reason)
            api.routes[('PUT',api.prefix+'/pulls/'+str(NUMBER)+'/merge')]=deny
            with self.subTest(reason=reason),self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=21)
            self.assertEqual(len(api.writes),1);self.assertEqual(api.writes[0][0],'PUT')

    def test_validation_can_collect_without_required_check_recursion(self):
        api,c,e=fixture();r=c.collect(NUMBER,require_checks=False)
        self.assertEqual(r['checks'],[]);self.assertEqual(api.writes,[])
        self.assertFalse(any('/actions/' in route for method,route,body in api.calls))

    def test_missing_api_fields_stops_and_partial_write_audit_is_honest(self):
        api,c,e=fixture(); del api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0]['head']
        with self.assertRaises(Stop): c.collect(NUMBER)
        api,c,e=fixture(True);publication_routes(api)
        api.add('/actions/workflows/guide-pages.yml/dispatches',None,method='POST')
        writer=Writer(c,e)
        with self.assertRaises(Stop): writer.execute(NUMBER,dry_run=False,trigger_run=21)
        self.assertEqual(writer.audit['merge_state'],'confirmed')
        self.assertEqual(writer.audit['merge_sha'],MERGED)
        self.assertEqual(writer.audit['dispatch_state'],'attempted_unconfirmed')

    def test_dispatch_unknown_failure_skip_wrong_run_or_timeout_no_success(self):
        for mutation in ['unknown','failed','skipped','wrong_sha','wrong_deployment','timeout']:
            api,c,e=fixture(True);publication_routes(api)
            run=api.routes[('GET',api.prefix+'/actions/runs/99')][0]
            if mutation=='unknown': api.add('/actions/workflows/guide-pages.yml/dispatches',None,method='POST')
            if mutation=='failed': run['conclusion']='failure'
            if mutation=='skipped': api.routes[('GET',api.prefix+'/actions/runs/99/attempts/1/jobs?per_page=100')][0]['jobs'][1]['conclusion']='skipped'
            if mutation=='wrong_sha': run['head_sha']='f'*40
            if mutation=='wrong_deployment': api.routes[('GET',api.prefix+'/deployments/5/statuses?per_page=100')][0][0]['log_url']='https://github.com/other/run'
            if mutation=='timeout': run['status']='in_progress'
            with self.subTest(mutation=mutation),self.assertRaises(Stop): Writer(c,e).execute(NUMBER,dry_run=False,trigger_run=21,poll=lambda:None,attempts=1)
            self.assertEqual(len(api.writes),2)

    def test_transport_never_writes_default_or_admin_endpoints(self):
        api=GitHub('hideki-ozu/DIR-Simulator',token='mock-only')
        with self.assertRaises(Stop): api.request('PUT',api.prefix+'/pulls/1000/merge',{})
        api.write_enabled=True
        with self.assertRaises(Stop): api.request('PUT',api.prefix+'/branches/main/protection',{})

    def test_real_pagination_implementation_two_pages_and_failures(self):
        api,c,e=fixture();route='/fake?per_page=100'
        api.add(route,[{'id':1}],'<https://api.github.com'+api.prefix+'/fake?per_page=100&page=2>; rel="next"')
        api.add('/fake?per_page=100&page=2',[{'id':2}])
        self.assertEqual(api.pages(route),[{'id':1},{'id':2}])
        api.add(route,[1],'<https://evil.test/fake?page=2>; rel="next"')
        with self.assertRaises(Stop): api.pages(route)
        api.add(route,[1,2]);
        with self.assertRaises(Stop): api.pages(route,cap=2)
        api.add(route,{'total_count':2,'rows':[1]})
        with self.assertRaises(Stop): api.pages(route,key='rows')
        api.add(route,list(range(100)))
        with self.assertRaises(Stop): api.pages(route)


if __name__=='__main__': unittest.main()
