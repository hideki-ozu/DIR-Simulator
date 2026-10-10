import unittest
import can_acceptance_matrix as m
class AcceptanceBindings(unittest.TestCase):
    def test_saved_source_and_named_results(self):
        d=m.build();self.assertEqual(len(d['cases']),13)
        self.assertEqual(sum(c['execution']['status']=='passed_limited' for c in d['cases']),8)
        self.assertEqual(sum(c['execution']['status']=='not_tested' for c in d['cases']),5)
        for c in d['cases']:
            if c['execution']['status']=='passed_limited': self.assertEqual(c['execution']['source_commit'],m.MEASURED)
    def test_no_overall_or_new_execution_approval(self):
        d=m.build();self.assertFalse(d['full_ac0008_acceptance']);self.assertFalse(d['product_executed_in_this_task'])
        self.assertTrue(d['historical_results_unchanged']);self.assertEqual(d['approval']['by'],'user')
    def test_generic_unbound_tests_stay_not_tested(self):
        d=m.build();r=next(c for c in d['cases'] if c['case_id']=='GENERIC-FACTORY-LIFECYCLE')
        self.assertEqual(r['execution']['status'],'not_tested');self.assertGreater(len(r['concrete_tests']),0)
    def test_observable_equality_does_not_prove_factory(self):
        for c in m.build()['cases'][:4]: self.assertIn('Observable equality is not factory/callback invocation evidence',c['limits'])
    def test_ids_unique_and_callback_positive_controls_real(self):
        r=m.build()['cases'];self.assertEqual(len(r),len({c['case_id'] for c in r}))
        for c in r:
            if c['case_id'].startswith('GENERIC-REJECTION'):
                self.assertEqual(c['execution']['positive_callback_count'],1);self.assertEqual(c['execution']['callback_count'],0)
    def test_builtin_workflow_does_not_inherit_generic_lifecycle(self):
        text=(m.ROOT/'docs/verification/cases/利用フロー・品質検証仕様書.md').read_text(encoding='utf-8')
        section=text.split('<a id="dir-test-0080"></a>',1)[1].split('<a id="dir-test-0081"></a>',1)[0]
        self.assertNotIn('runは全factory/initialize成功後',section)
        self.assertIn('CAN Engineの初期状態',section)
        self.assertIn('custom Registryと実記録hookを持つ独立fixture',text)
if __name__=='__main__': unittest.main()
