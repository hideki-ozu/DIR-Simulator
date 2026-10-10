import json, unittest
from pathlib import Path
OUT=Path(__file__).resolve().parents[2]/'docs/verification/results/ac0008-review-2026-10-10'
class PacketTests(unittest.TestCase):
    def read(self,n): return json.loads((OUT/n).read_text(encoding='utf-8'))
    def test_rule_ids_unique(self):
        r=self.read('ned-rule-matrix.json'); rows=r['rules']; self.assertEqual(r['core_group_count'],37)
        self.assertEqual(len(rows),len({x['rule_id'] for x in rows}))
        self.assertTrue(all(x['status'] in ('mismatch','not_tested') and x['reviewer'] is None for x in rows))
    def test_all_ten_case_groups_and_unique_ids(self):
        rows=self.read('ned-subcase-matrix.json')['cases']
        self.assertEqual({x['dir_anchor_clause']['anchor'] for x in rows},{f'dir-test-{n:04d}' for n in range(60,70)})
        self.assertEqual(len(rows),len({x['rule_id'] for x in rows}))
    def test_no_bulk_approval_or_or_selection(self):
        d=self.read('ledger-review-proposals.json');r=d['rows'];self.assertEqual(len(r),50)
        self.assertEqual(sum(x['kind']=='Cargo' for x in r),42)
        self.assertEqual(len(r),len({x['key'] for x in r}))
        self.assertTrue(all(x['approval']=='unapproved' and x['proposed_route'] is None for x in r))
        self.assertEqual(len(d['additional_subcomponents']),1)
    def test_origins_require_attestation(self):
        d=self.read('origin-attestation-packet.json');self.assertFalse(d['author_attestation_received'])
        self.assertFalse(d['removed_omnet_tools_reintroduced'])
        self.assertTrue(all(x['author_attestation'] is None for x in d['files']))
    def test_atomic_oracle_ids_and_37_groups(self):
        d=self.read('atomic-oracles.json');r=d['core_atoms']
        self.assertEqual(len({x['group_id'] for x in r}),37)
        self.assertEqual(len(r),len({x['case_id'] for x in r}))
        self.assertEqual(d['execution_passes'],0)
        self.assertTrue(all(x['status'] in ('mismatch','not_tested') and x['execution_evidence'] is None for x in r))
    def test_source_subcase_rows_are_all_retained(self):
        import ac0008_matrix as m
        expected={(r['anchor'],r['line']) for r in m.table_rows(m.CASES) if r['anchor'] in {f'dir-test-{n:04d}' for n in range(60,70)}}
        r=self.read('atomic-oracles.json')['subcase_oracles']
        actual={(x['dir_clause']['anchor'],x['dir_clause']['line']) for x in r}
        self.assertEqual(expected,actual)
        self.assertEqual(len(r),len({x['case_id'] for x in r}))
    def test_every_static_binding_names_a_real_assertion_catalogue_test(self):
        cat=self.read('test-assertion-catalogue.json')['tests'];ids={t['test_id'] for t in cat}
        self.assertTrue(all(t['assertions'] or t['assertion_status'].startswith('no_direct_assertion') for t in cat))
        for r in self.read('atomic-oracles.json')['core_atoms']:
            for ref in r['existing_concrete_tests']+r['exact_static_test_bindings']: self.assertIn(ref['test_id'],ids)
    def test_all_unbound_predicates_have_named_gaps(self):
        r=self.read('atomic-oracles.json')['core_atoms']
        self.assertEqual({x['case_id'] for x in r if not x['exact_static_test_bindings']},{x['case_id'] for x in self.read('predicate-binding-gaps.json')['core_gaps']})
    def test_numeric_variants_have_specific_oracles(self):
        r=[x for x in self.read('atomic-oracles.json')['subcase_oracles'] if x['dir_clause']['anchor']=='dir-test-0067']
        self.assertTrue(all(x['expected_observable'].startswith(('受理','拒否')) for x in r))
        self.assertEqual(len(r),len({(x['dir_clause']['line'],x['operation']) for x in r}))
if __name__=='__main__': unittest.main()
