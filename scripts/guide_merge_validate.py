"""Validate data-only overlays using trusted-base tools; never execute PR scripts."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import warnings
import zlib
from guide_scope_policy import ALLOWLIST, IMAGES, Stop, need
from guide_merge_runtime import Collector, GitHub, load_policy


def validate_png(data):
    from PIL import Image
    need(data.startswith(b'\x89PNG\r\n\x1a\n'), 'png_signature')
    offset, kinds, idat_ended = 8, [], False
    allowed = {b'IHDR', b'PLTE', b'IDAT', b'IEND', b'tRNS', b'sRGB', b'gAMA', b'cHRM', b'pHYs'}
    while offset < len(data):
        need(offset + 12 <= len(data), 'png_truncated')
        size = struct.unpack('>I', data[offset:offset+4])[0]
        kind = data[offset+4:offset+8]
        need(size <= 2_000_000 and offset + size + 12 <= len(data) and kind in allowed, 'png_chunk_or_size')
        payload = data[offset+8:offset+8+size]
        crc = struct.unpack('>I', data[offset+8+size:offset+12+size])[0]
        need(zlib.crc32(kind + payload) & 0xffffffff == crc, 'png_crc')
        if kind == b'IDAT':
            need(not idat_ended, 'png_nonconsecutive_idat')
        elif b'IDAT' in kinds:
            idat_ended = True
        kinds.append(kind); offset += size + 12
        if kind == b'IEND':
            need(size == 0 and offset == len(data), 'png_trailing_payload')
            break
    need(kinds and kinds[0] == b'IHDR' and kinds[-1] == b'IEND' and kinds.count(b'IHDR') == 1 and
         kinds.count(b'IEND') == 1 and b'IDAT' in kinds, 'png_structure')
    with warnings.catch_warnings():
        warnings.simplefilter('error')
        with Image.open(io.BytesIO(data)) as source:
            need(source.format == 'PNG' and not getattr(source, 'is_animated', False) and
                 source.width * source.height <= 10_000_000 and source.width > 0 and source.height > 0, 'png_dimensions_or_animation')
            source.verify()
        with Image.open(io.BytesIO(data)) as source:
            source.load(); normalized = source.convert('RGBA')
            clean = io.BytesIO(); normalized.save(clean, format='PNG')
            encoded = clean.getvalue()
            with Image.open(io.BytesIO(encoded)) as decoded:
                decoded.load()
                need(decoded.size == normalized.size and decoded.tobytes() == normalized.tobytes(), 'png_reencode_pixel_mismatch')
    return encoded


def validate_overlay(root, files, binary, runner=subprocess.run):
    root, binary = Path(root).resolve(), Path(binary).resolve()
    need(binary.is_file(), 'trusted_binary_missing')
    need(set(files) <= ALLOWLIST and files, 'overlay_outside_allowlist')
    # Trusted checkout contains code/dependencies; PR input is only bounded bytes.
    with tempfile.TemporaryDirectory(prefix='dir-guide-validation-') as scratch:
        scratch = Path(scratch)
        work = scratch / 'repo'
        shutil.copytree(root, work, ignore=shutil.ignore_patterns('.git', 'target', 'build', 'guide-evidence', '__pycache__', '.venv*', '.codex', '.agents', '.aws'))
        for relative, data in files.items():
            path = work / relative
            need(path.resolve().is_relative_to(work) and not path.is_symlink() and
                 not any(p.is_symlink() for p in path.parents if p != work.parent), 'overlay_symlink')
            path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(data)
        images = {}
        for path in sorted(IMAGES):
            source = work / path
            need(source.is_file(), 'public_image_missing')
            raw = source.read_bytes(); clean = validate_png(raw)
            # Re-encoded image is used only in the isolated preview/build.
            source.write_bytes(clean)
            images[path] = {'original_sha256': hashlib.sha256(raw).hexdigest(),
                            'reencoded_sha256': hashlib.sha256(clean).hexdigest()}
        env = {k: v for k, v in os.environ.items()
               if k in {'PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'TMPDIR', 'PYTHONPATH', 'LANG', 'LC_ALL'}}
        if (root / '.git').exists():
            # Existing reproduction tool reads only the source revision. Its Git
            # metadata stays in the trusted checkout, not in the PR-data overlay.
            metadata = runner(['git', '-C', str(root), 'rev-parse', '--absolute-git-dir'],
                              env=env, capture_output=True, text=True, timeout=10, check=False)
            need(metadata.returncode == 0, 'trusted_git_revision_unavailable')
            env['GIT_DIR'] = metadata.stdout.strip()
            env['GIT_WORK_TREE'] = str(root)
        commands = [[sys.executable, 'scripts/build_guide_inputs.py', '--check'],
                    [sys.executable, '-m', 'mkdocs', 'build', '--strict'],
                    [sys.executable, 'scripts/check_guide_links.py'],
                    [sys.executable, 'scripts/verify_guide.py', '--binary', str(binary), '--output', str(scratch / 'reproduce')]]
        for argv in commands:
            result = runner(argv, cwd=work, env=env, capture_output=True, text=True, timeout=120, check=False)
            need(result.returncode == 0, 'trusted_validation_command_failed_' + Path(argv[1]).name)
        return {'status': 'passed', 'images_decoded_reencoded': len(images), 'images': images,
                'checks': ['sample_zip_match', 'mkdocs_strict', 'public_assets_links', 'three_sample_reproductions'],
                'pr_code_executed': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pr', type=int, required=True)
    parser.add_argument('--trusted-sha', required=True)
    parser.add_argument('--root', default='.')
    parser.add_argument('--binary', required=True)
    args = parser.parse_args()
    try:
        policy = load_policy(Path(args.root) / '.github/guide-merge-policy.json')
        snapshot = Collector(GitHub(policy['repository'], os.environ.get('GH_TOKEN', '')), policy, args.trusted_sha).collect(args.pr, require_checks=False)
        result = validate_overlay(args.root, snapshot['files'], args.binary)
        result.update(head_sha=snapshot['head_sha'], base_sha=snapshot['base_sha'])
        print(json.dumps(result, ensure_ascii=False)); return 0
    except (Stop, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as e:
        print(json.dumps({'stopped': True, 'reason': str(e) if isinstance(e, Stop) else 'validation_failed_or_unavailable'})); return 2


if __name__ == '__main__':
    raise SystemExit(main())
