# Project conventions

- `docs/要件定義書.md` owns requirement, out-of-scope, acceptance, and TBD IDs. Preserve IDs and their meaning; do not hide incomplete branches.
- Traceability follows requirements → functional spec → detailed functional spec → architecture → detailed design → verification. Constraint exceptions require their documented review route.
- Generated traceability Markdown/HTML are outputs; update them with `scripts/generate_traceability.py`, never by hand.
- `.puml` is the diagram source and `.svg` is generated output. Update both together; do not edit SVG directly.
- NED support is defined from public specifications. Keep independent implementation provenance and do not copy OMNeT++/INET implementation code.