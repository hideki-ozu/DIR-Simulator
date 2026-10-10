"""Generate reviewer proposals; no license route or test outcome is approved here."""
from pathlib import Path
import hashlib, json, re
ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'docs/verification/results/ac0008-review-2026-10-10'
def save(name,data):
    (OUT/name).write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n',encoding='utf-8',newline='\n')
def main():
    ledger=next((ROOT/'docs/third-party').glob('*台帳.md'))
    rows=[]; cargo=True
    for line,text in enumerate(ledger.read_text(encoding='utf-8').splitlines(),1):
        if text.startswith('## ガイド'): cargo=False
        if not text.startswith('| ') or text.startswith('| ---'): continue
        cells=[s.strip() for s in text.strip('|').split('|')]
        if len(cells) not in (8,12) or cells[0]=='名前': continue
        name=cells[0].strip('`');version=cells[1].strip('`')
        if cargo and len(cells)!=12: continue
        if not cargo and len(cells)!=8: continue
        spdx=cells[6] if cargo else cells[4]
        rows.append(dict(key=('cargo:' if cargo else 'guide:')+name+'@'+version,kind='Cargo' if cargo else 'guide',ledger_line=line,ledger_cells=cells,
          source_population='fixed Cargo.lock registry package' if cargo else 'fixed guide ledger asset',archive_membership='pending exact member-level binding; prior hash reconciliation preserved',
          binary_membership='unknown: exact WSL target/features/build graph/link evidence missing',proposed_route=None,route_options=spdx,approval='unapproved',reviewer=None,
          conditions=['Preserve applicable license text, copyright and notices','If MIT chosen: include permission/copyright notice in copies or substantial portions','If Apache-2.0 chosen: preserve license, applicable NOTICE and mark modifications; no trademark grant'] if cargo else ['Apply file-specific license to actual distributed asset; preserve notice and modification/origin evidence'],
          source_and_license_evidence='existing ledger links and fixed original reconciliation; no archive/crate re-download',deficits=['actual distribution membership','author/adoption confirmation','target build/link evidence'] if cargo else ['actual final asset membership','origin/adoption confirmation'],
          proposal_prepared_by='Codex; not legal or adoption approval'))
    assert len(rows)==50 and sum(r['kind']=='Cargo' for r in rows)==42,len(rows)
    save('ledger-review-proposals.json',dict(schema_version=1,ledger_version='1.1.1',rows=rows,bulk_approval=False,
      additional_subcomponents=[dict(key='guide-subcomponent:UMD@returnExports',parent_keys=['guide:lunr-languages@1.12.0','guide:TinySegmenter@0.1'],status='provenance_pending',proposed_route='MIT preservation proposal only',approval='unapproved',pinned_commit='66b7e3b35488828d32d3b5f30f5e2cee035fdfa0',license_blob='6393241af4c07ba65c6d68cee358a1e20bacce23',license_sha256='5485746dcb95d5e00d18b9ea30571345dea7b61849d53835b139c526a903948d',wrapper_blob='221974c07c5970430fc4b50d0f0ff459d2d88af7',adapted_wrapper_evidence={'lunr.ja.js':'lines18–38','lunr.stemmer.support.js':'lines10–29','tinyseg.js':'lines1–20'},deficits=['Exact copied upstream revision and author attestation','Final distribution notice integration'],byte_identity_claim=False)],
      blockers={'Snowball_v0_3':'Oleg Mazko 2010 / Urim MPL-1.1 header; source origin unresolved; modern BSD does not resolve this version','rustix_CC0':'Standalone CC0 text unavailable after HTTP403; no retry or alternate route attempted','environment':'WSL build/link/system library and Python wheel/Playwright distribution population evidence missing'},
      primary_condition_sources=['https://opensource.org/license/mit','https://www.apache.org/licenses/LICENSE-2.0','https://www.mozilla.org/en-US/MPL/1.1/']))
    tree=json.loads((OUT/'fixed-tree.json').read_text(encoding='utf-8'))
    histories=json.loads((OUT/'origin-history.json').read_text(encoding='utf-8'))
    introductions=json.loads((OUT/'origin-introductions.json').read_text(encoding='utf-8'))
    bypath={h['path']:h for h in histories if not h['directory_query']}
    items=[]
    for item in tree['tree']:
        p=item['path']
        if item['type']!='blob': continue
        if not (p.endswith(('.ned','.ini')) or '/tests' in p or '/fixtures/' in p or p.startswith('docs/diagrams/') or p.startswith('scripts/generate_') or p in ('crates/dir-simulator/src/input.rs','crates/dir-simulator/src/input/ned.rs')): continue
        h=bypath.get(p);candidate=h['introduction_candidate'] if h else None
        added=any(c['sha']==candidate and any(f['path']==p and f['status']=='added' for f in c['files']) for c in introductions)
        items.append(dict(key=p,fixed_blob=item['sha'],history_reference=p if h else None,introduction_commit=candidate if added else None,introduction_status='added status confirmed in available history' if added else 'pending file-specific introduction query',origin_status='provenance_pending',author_attestation=None,
          author_question='Confirm original author(s), independent creation vs adapted source, exact upstream revision/license/changes if copied, fixture/generated input origins and tool/font sources. Git authorship and authored parser alone are insufficient.',generated_origin='Record generator/version/input hashes and any template/font source' if p.startswith('docs/diagrams/') else None))
    save('origin-attestation-packet.json',dict(schema_version=1,files=items,scope='fixed-tree tracked NED/INI/tests/fixtures/generated diagrams and parser; not an exhaustive archive license audit',removal_commits=['6a3ab59d826e8d4989fae9a031a3d317fb06ce80','51e9ef36fbdb3172b4d1870e058e15517fec40bf','aa8ffe512d9e04cf66a69e230c2ea321a620f6c1'],removed_omnet_tools_reintroduced=False,author_attestation_received=False))
    print(f'{len(rows)} ledger proposals; {len(items)} origin items; all approvals pending')
if __name__=='__main__': main()
