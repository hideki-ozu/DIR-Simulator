"""Issue31 evidence tests. All API responses and writes are synthetic."""
import io
import json
from urllib.error import HTTPError
import unittest
from unittest.mock import patch
from guide_merge_runtime import PublicDeployments, Writer, Stop, deployment_preflight
from test_guide_merge_runtime import fixture, publication_routes, BASE, MERGED, NUMBER


class Response(io.BytesIO):
    def __init__(self, data, link=''):
        super().__init__(json.dumps(data).encode())
        self.headers = {'Link':link}


class EvidenceTests(unittest.TestCase):
    def test_public_transport_never_sends_token_or_reads_rules(self):
        api = PublicDeployments('hideki-ozu/DIR-Simulator')
        requests = []
        def opened(req, **kwargs):
            requests.append(req)
            return Response([])
        with patch('guide_merge_runtime.build_opener') as opener, patch.dict('os.environ', {'GH_TOKEN':'mock-only','GITHUB_TOKEN':'mock-only'}):
            opener.return_value.open.side_effect = opened
            self.assertEqual(api.pages('/deployments?sha='+BASE+'&environment=github-pages&per_page=100'), [])
            self.assertNotIn('Authorization', dict(requests[0].header_items()))
            self.assertEqual(requests[0].get_method(), 'GET')
            for method, path in [('GET','/rules/branches/main?per_page=100'),
                                 ('GET','/rulesets/1'),('POST','/deployments'),
                                 ('PUT','/pulls/1000/merge'),('GET','/actions/runs/1'),
                                 ('GET','/deployments/0/statuses?per_page=100')]:
                with self.subTest(path=path), self.assertRaises(Stop):
                    api.request(method, api.prefix+path)
            self.assertEqual(len(requests), 1)
            api.token='mock-only'
            with self.assertRaises(Stop): api.pages('/deployments/1/statuses?per_page=100')
            self.assertEqual(len(requests), 1)

    def test_query_scope_and_pagination_complete_without_authentication(self):
        api=PublicDeployments('hideki-ozu/DIR-Simulator')
        route=api.prefix+'/deployments?sha='+BASE+'&environment=github-pages&per_page=100'
        def opened(req, **kwargs):
            self.assertNotIn('Authorization', dict(req.header_items()))
            if 'page=2' in req.full_url: return Response([{'id':2}])
            return Response([{'id':1}], '<https://api.github.com'+route+'&page=2>; rel="next"')
        with patch('guide_merge_runtime.build_opener') as opener:
            opener.return_value.open.side_effect=opened
            self.assertEqual(api.pages(route[len(api.prefix):]), [{'id':1},{'id':2}])
            for query in ['per_page=100','sha='+BASE+'&environment=production&per_page=100',
                          'sha='+BASE+'&environment=github-pages&per_page=100&token=x',
                          'sha='+BASE+'&environment=github-pages&per_page=100&page=101',
                          'sha='+BASE+'&environment=github-pages&per_page=100&per_page=100']:
                with self.subTest(query=query), self.assertRaises(Stop):
                    api.pages('/deployments?'+query)

    def test_public_read_denial_never_retries_or_falls_back(self):
        api=PublicDeployments('hideki-ozu/DIR-Simulator')
        for code in [403,404,429]:
            with patch('guide_merge_runtime.build_opener') as opener:
                opener.return_value.open.side_effect=HTTPError('https://api.github.com/public', code, 'mock', {}, None)
                with self.subTest(code=code), self.assertRaisesRegex(Stop, 'api_http_'+str(code)):
                    api.pages('/deployments?sha='+BASE+'&environment=github-pages&per_page=100')
                self.assertEqual(opener.return_value.open.call_count, 1)

    def test_preflight_failures_stop_before_any_merge_or_dispatch(self):
        for mutation in ['missing','wrong_sha','wrong_environment','duplicate','missing_status','failed_status','denied']:
            api,c,e=fixture(True); publication_routes(api)
            key=('GET',api.prefix+'/deployments?sha='+BASE+'&environment=github-pages&per_page=100')
            rows=api.routes[key][0]
            if mutation=='missing': rows.clear()
            if mutation=='wrong_sha': rows[0]['sha']='f'*40
            if mutation=='wrong_environment': rows[0]['environment']='other'
            if mutation=='duplicate': rows*=2
            status_key=('GET',api.prefix+'/deployments/4/statuses?per_page=100')
            if mutation=='missing_status': api.routes[status_key][0].clear()
            if mutation=='failed_status': api.routes[status_key][0][0]['state']='failure'
            if mutation=='denied':
                def deny(_): raise Stop('api_http_403')
                api.routes[key]=deny
            writer=Writer(c,e,publication_api=api)
            with self.subTest(mutation=mutation), self.assertRaises(Stop):
                writer.execute(NUMBER,dry_run=False,trigger_run=21)
            self.assertEqual(api.writes, [])
            self.assertEqual(writer.audit['merge_state'], 'not_attempted')

    def test_public_evidence_is_separate_from_writer_credential(self):
        api,c,e=fixture(True); publication_routes(api)
        public,c2,e2=fixture(True); publication_routes(public)
        # Authenticated writer has no deployment routes; there is no fallback.
        for key in list(api.routes):
            if '/deployments' in key[1]: del api.routes[key]
        writer=Writer(c,e,publication_api=public)
        result=writer.execute(NUMBER,dry_run=False,trigger_run=21,poll=lambda:None)
        self.assertEqual(result['publication'], 'success')
        self.assertFalse(any('/deployments' in route for method,route,body in api.calls))
        self.assertEqual(public.writes, [])
        self.assertEqual(writer.audit['public_deployment_preflight']['source'], 'github_public_rest_no_credentials')

    def test_bypass_provenance_and_visibility_stop_with_specific_reason(self):
        for mutation,reason in [('missing','bypass_evidence_not_visible'),('null','bypass_evidence_malformed'),
                                ('present','bypass_actors_present'),('identity','ruleset_identity_mismatch'),
                                ('boolean_id','ruleset_identity_mismatch'),('source','ruleset_identity_mismatch')]:
            api,c,e=fixture(True)
            ruleset=api.routes[('GET',api.prefix+'/rulesets/1?includes_parents=true')][0]
            if mutation=='missing': del ruleset['bypass_actors']
            if mutation=='null': ruleset['bypass_actors']=None
            if mutation=='present': ruleset['bypass_actors']=[{'actor_id':1}]
            if mutation=='identity': ruleset['id']=2
            if mutation=='boolean_id': ruleset['id']=True
            if mutation=='source': ruleset['source']='other/repository'
            writer=Writer(c,e,publication_api=api)
            with self.subTest(mutation=mutation), self.assertRaisesRegex(Stop,reason):
                writer.execute(NUMBER,dry_run=False,trigger_run=21)
            self.assertFalse(any('/deployments' in route for method,route,body in api.calls))
            self.assertEqual(api.writes, [])

    def test_postmerge_public_denial_retains_partial_success_audit(self):
        api,c,e=fixture(True); publication_routes(api)
        public,c2,e2=fixture(True); publication_routes(public)
        def deny(_): raise Stop('api_http_403')
        public.routes[('GET',public.prefix+'/deployments?sha='+MERGED+'&environment=github-pages&per_page=100')]=deny
        writer=Writer(c,e,publication_api=public)
        with self.assertRaisesRegex(Stop,'api_http_403'):
            writer.execute(NUMBER,dry_run=False,trigger_run=21,poll=lambda:None)
        self.assertEqual(writer.audit['merge_state'],'confirmed')
        self.assertEqual(writer.audit['merge_sha'],MERGED)
        self.assertEqual(writer.audit['dispatch_state'],'run_confirmed')
        self.assertEqual(len(api.writes),2)
        self.assertEqual(public.writes,[])


if __name__ == '__main__': unittest.main()
