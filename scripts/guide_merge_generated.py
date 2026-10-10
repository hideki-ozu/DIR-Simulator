"""Prove derived reports by byte equality with trusted checkout generation."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from guide_scope_policy import ALLOWLIST, GENERATED_REPORTS, GUIDES, need, Stop


def verify_generated(root, files, runner=subprocess.run, trusted_sha=None):
    root = Path(root).resolve()
    need(files and set(files) <= ALLOWLIST | GENERATED_REPORTS, 'generated_overlay_scope')
    # A report-only PR cannot independently edit the document inventory.
    if set(files) & GENERATED_REPORTS:
        need(set(files) & GUIDES, 'generated_without_guide_change')
    env = {k: v for k, v in os.environ.items()
           if k in {'PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'TMPDIR', 'LANG', 'LC_ALL'}}
    env['PYTHONIOENCODING'] = 'utf-8'
    if trusted_sha is not None:
        revision = runner(['git', '-C', str(root), 'rev-parse', 'HEAD'], env=env,
                          capture_output=True, timeout=10, check=False)
        clean = runner(['git', '-C', str(root), 'status', '--porcelain', '--untracked-files=all',
                        '--', 'docs', 'scripts'], env=env, capture_output=True, timeout=10, check=False)
        need(revision.returncode == 0 and revision.stdout.strip() == trusted_sha.encode() and
             clean.returncode == 0 and not clean.stdout.strip(), 'generated_base_checkout_untrusted')
    with tempfile.TemporaryDirectory(prefix='dir-guide-generated-') as tmp:
        work = Path(tmp)
        shutil.copytree(root / 'docs', work / 'docs')
        for relative, data in files.items():
            if relative.startswith('docs/'):
                path = work / relative
                need(not path.is_symlink() and path.resolve().is_relative_to(work), 'generated_overlay_symlink')
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
        # Save both reports, including an unchanged counterpart. Missing/stale
        # output is a stop; the generator also writes an out-of-scope hierarchy
        # annex which must remain byte-identical to trusted base.
        outputs = GENERATED_REPORTS | {'docs/要件階層.html'}
        before = {p: files[p] if p in files else (root / p).read_text(encoding='utf-8').encode('utf-8')
                  for p in outputs}
        try:
            result = runner([sys.executable, str(root / 'scripts/generate_traceability.py'),
                             '--root', str(work)], cwd=work, env=env,
                            capture_output=True, timeout=120, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise Stop('trusted_generation_unavailable') from error
        need(result.returncode == 0, 'trusted_generation_failed')
        # Generator rendering uses LF. Windows text-mode writes may translate
        # those newlines; canonicalize only trusted output, never PR bytes.
        need(all((work / p).read_text(encoding='utf-8').encode('utf-8') == data for p, data in before.items()),
             'generated_report_mismatch')
    return True
