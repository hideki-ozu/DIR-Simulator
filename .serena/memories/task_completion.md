# Completion checks

- For changes to requirement/document traceability inputs, run `python3 scripts/generate_traceability.py`, then `python3 scripts/check_traceability.py`; review links, IDs, and generated diffs.
- For diagram-source changes, run `python3 scripts/render_diagrams.py`, then `python3 scripts/render_diagrams.py --check`.
- For changes to Python maintenance tools or hooks, run `python3 -m unittest discover -s tests` when tests are requested or required by the task's validation instructions.
- The whole-project `python3 scripts/check_traceability.py --strict` is intentionally incomplete while planned documents are still drafts; use scoped strict checks only for a completed implementation slice.