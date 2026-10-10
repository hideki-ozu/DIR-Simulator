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
if __name__=='__main__': unittest.main()
