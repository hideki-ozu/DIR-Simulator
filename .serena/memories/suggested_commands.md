# Commands from the repository root

- `python3 scripts/check_traceability.py` reports traceability consistency and remaining incomplete paths.
- `python3 scripts/check_traceability.py --impact DIR-FUNC-008` inspects downstream impact for an ID; replace the ID as needed.
- `python3 scripts/generate_traceability.py` updates both generated traceability reports.
- `python3 scripts/render_diagrams.py` regenerates committed SVGs from PlantUML sources; `--check` compares outputs without replacing them.
- `python3 -m unittest discover -s tests` runs the standard-library test suite for the maintenance scripts and hook.
- `--strict` traceability checks currently report unfinished documentation paths; do not interpret that expected draft state as a clean full-project trace.