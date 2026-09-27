#!/usr/bin/env python3
"""Validate DIR trace metadata; it never executes metadata."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict, deque
from dataclasses import dataclass
from pathlib import Path

REQ = re.compile(r"\*\*(DIR-REQ-\d{3})\*\*", re.I)
RREQ = re.compile(r"DIR-REQ-\d{3}", re.I)
RFUNC = re.compile(r"DIR-FUNC-(\d{3})", re.I)
RAC = re.compile(r"DIR-AC-\d{3}", re.I)
ANCHOR = re.compile(r'<a\s+id=["\']([^"\']+)["\']\s*></a>', re.I)
DOCID = re.compile(r"^文書ID：[ \t]*`([^`\n]+)`[ \t]*$", re.M)
FENCE = re.compile(r"^```([^\n]*)\n(.*?)^```\s*$", re.M | re.S)
STAGES = ("spec", "architecture", "design", "verification")
NEXT = dict(zip(STAGES, STAGES[1:]))
GRAM = re.compile(r"[a-z0-9-]+$")


@dataclass
class Node:
    id: str
    stage: str
    requirements: list[str]
    upstream: list[str]
    state: str
    pending: list[str]
    path: Path


def C(x):
    return x.upper()


def unfenced(x):
    return FENCE.sub("", x)


def blocks(x, kind):
    for m in FENCE.finditer(x):
        if m.group(1).strip() == kind:
            yield m.group(2)


def obj(raw):
    def hook(items):
        out = {}
        for k, v in items:
            if k in out:
                raise ValueError(f"duplicate JSON key {k!r}")
            out[k] = v
        return out

    return json.loads(raw, object_pairs_hook=hook)


def funcs(cell):
    out = {int(x) for x in RFUNC.findall(cell)}
    for a, b in re.findall(
        r"DIR-FUNC-(\d{3}).{0,80}?[〜~].{0,80}?DIR-FUNC-(\d{3})", cell, re.I
    ):
        out.update(range(min(int(a), int(b)), max(int(a), int(b)) + 1))
    return {f"DIR-FUNC-{x:03d}" for x in out}


class Check:
    def __init__(self, root):
        self.root = root
        self.docs = root / "docs"
        self.errors = []
        self.requirements = set()
        self.functions = set()
        self.req_funcs = defaultdict(set)
        self.req_titles = {}
        self.req_acs = defaultdict(set)
        self.exceptions = set()
        self.nodes = {}
        self.anchors = {}
        self.docids = {}
        self.cyclic = False

    def err(self, x):
        self.errors.append(x)

    def label(self, p):
        return str(p.relative_to(self.root))

    def read(self, p):
        try:
            return p.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as e:
            self.err(f"{self.label(p)}: cannot read: {e}")
            return ""

    def sources(self):
        rp, fp = self.docs / "要件定義書.md", self.docs / "機能仕様書.md"
        rt, ft = self.read(rp), self.read(fp)
        counts = defaultdict(int)
        for x in REQ.findall(unfenced(rt)):
            counts[C(x)] += 1
        self.requirements = set(counts)
        if not self.requirements:
            self.err("docs/要件定義書.md: no canonical requirements")
        for x, n in counts.items():
            if n != 1:
                self.err(f"docs/要件定義書.md: duplicate canonical requirement {x}")
        requirement_anchors = set(ANCHOR.findall(unfenced(rt)))
        for requirement in self.requirements:
            if requirement.lower() not in requirement_anchors:
                self.err(f"{requirement}: missing explicit requirement anchor")
        fa = set(ANCHOR.findall(unfenced(ft)))
        self.functions = {
            anchor.upper() for anchor in fa if re.fullmatch(r"dir-func-\d{3}", anchor)
        }
        if not self.functions:
            self.err("docs/機能仕様書.md: no explicit dir-func-NNN anchors")
        start = re.search(r"^### 11\.2\..*$", ft, re.M)
        if not start:
            self.err("docs/機能仕様書.md: missing §11.2 mapping table")
        else:
            body = ft[start.end() :]
            stop = re.search(r"^#{1,3}\s+", body, re.M)
            rows = defaultdict(int)
            for line in body[: stop.start() if stop else len(body)].splitlines():
                if not line.lstrip().startswith("|"):
                    continue
                cells = line.split("|")
                if len(cells) < 4:
                    continue
                found = {C(x) for x in RREQ.findall(cells[1])}
                if not found:
                    continue
                if len(found) != 1:
                    self.err("§11.2: mapping row must identify one requirement")
                    continue
                r = found.pop()
                rows[r] += 1
                if len(cells) > 2:
                    self.req_titles[r] = cells[2].strip()
                mapped = funcs(cells[3])
                self.req_funcs[r].update(mapped)
                ac_cell = cells[-2] if cells[-1].strip() == "" else cells[-1]
                self.req_acs[r].update(C(x) for x in RAC.findall(ac_cell))
                if not mapped:
                    self.err(f"§11.2: {r} has no functions in function column")
            for r in self.requirements:
                if rows[r] != 1:
                    self.err(f"§11.2: {r} has {rows[r]} mapping rows")
            for r in rows:
                if r not in self.requirements:
                    self.err(f"§11.2: unknown requirement {r}")
            for r, fs in self.req_funcs.items():
                for f in fs:
                    if f not in self.functions:
                        self.err(f"§11.2: {r} references unknown function {f}")
            for f in self.functions:
                if not any(f in fs for fs in self.req_funcs.values()):
                    self.err(f"orphan function {f} has no §11.2 requirement")
        for raw in blocks(rt, "trace-exception"):
            try:
                d = obj(raw)
            except (ValueError, json.JSONDecodeError) as e:
                self.err(f"docs/要件定義書.md: malformed trace-exception JSON: {e}")
                continue
            if not isinstance(d, dict) or set(d) != {"requirement", "reason", "stages"}:
                self.err("docs/要件定義書.md: invalid trace-exception fields")
                continue
            r = d["requirement"]
            if (
                not isinstance(r, str)
                or C(r) not in self.requirements
                or C(r) in self.exceptions
            ):
                self.err(
                    "docs/要件定義書.md: exception requirement must be unique and known"
                )
            elif (
                not isinstance(d["reason"], str)
                or not d["reason"].strip()
                or d["stages"] != ["requirement", "verification"]
            ):
                self.err(f"docs/要件定義書.md: invalid exception {r}")
            else:
                self.exceptions.add(C(r))

    def nodes_load(self):
        paths = sorted(
            p
            for p in self.docs.rglob("*.md")
            if "templates" not in p.relative_to(self.docs).parts
            and p
            not in (
                self.docs / "初期設計構想文書.md",
                self.docs / "要件トレーサビリティ一覧.md",
            )
        )
        for p in paths:
            text = self.read(p)
            clean = unfenced(text)
            lab = self.label(p)
            found = ANCHOR.findall(clean)
            self.anchors[p] = set(found)
            for x in set(found):
                if found.count(x) > 1:
                    self.err(f"{lab}: duplicate explicit anchor {x}")
            ids = DOCID.findall(clean)
            rel = p.relative_to(self.docs).as_posix()
            if len(ids) != 1:
                self.err(f"{lab}: exactly one document ID is required")
            if ids:
                did = ids[0]
                if not GRAM.fullmatch(did):
                    self.err(f"{lab}: invalid document ID {did}")
                elif did in self.docids:
                    self.err(
                        f"duplicate document ID {did}: {lab} and {self.label(self.docids[did])}"
                    )
                else:
                    self.docids[did] = p
            trace = list(blocks(text, "trace"))
            if (
                rel.startswith(("specs/", "design/", "verification/cases/"))
                and not trace
            ):
                self.err(f"{lab}: normative stage document has no trace node")
            for raw in trace:
                try:
                    self.add(p, obj(raw))
                except (ValueError, json.JSONDecodeError) as e:
                    self.err(f"{lab}: malformed trace JSON: {e}")
            if p != self.docs / "要件定義書.md" and any(
                True for _ in blocks(text, "trace-exception")
            ):
                self.err(f"{lab}: trace-exception only allowed in 要件定義書.md")

    def add(self, p, d):
        lab = self.label(p)
        fields = {"id", "stage", "requirements", "upstream", "state", "pending"}
        if not isinstance(d, dict) or set(d) != fields:
            self.err(f"{lab}: trace fields invalid")
            return
        ident, stage = d["id"], d["stage"]
        if not isinstance(ident, str) or stage not in STAGES:
            self.err(f"{lab}: invalid trace id or stage")
            return
        if not all(
            isinstance(d[x], list) and all(isinstance(v, str) for v in d[x])
            for x in ("requirements", "upstream", "pending")
        ):
            self.err(f"{lab}: requirements, upstream, pending must be string lists")
            return
        for k in ("requirements", "upstream", "pending"):
            if any(not x.strip() for x in d[k]) or len(set(d[k])) != len(d[k]):
                self.err(f"{lab}: {k} cannot contain blank or duplicate values")
        if not d["requirements"]:
            self.err(f"{lab}: requirements must be nonempty")
        if d["state"] not in ("draft", "confirmed"):
            self.err(f"{lab}: state must be draft or confirmed")
        if d["state"] == "confirmed" and d["pending"]:
            self.err(f"{lab}: confirmed node cannot have pending reasons")
        if not d["upstream"] and not d["pending"]:
            self.err(f"{lab}: upstream=[] requires a nonempty pending reason")
        doc = DOCID.search(unfenced(self.read(p)))
        anchor = ""
        if stage == "verification":
            valid = bool(re.fullmatch(r"DIR-TEST-\d{3}", ident))
            anchor = ident.lower()
        else:
            valid = bool(
                doc and "#" in ident and ident.split("#", 1)[0] == doc.group(1)
            )
            anchor = ident.split("#", 1)[1] if "#" in ident else ""
        if stage != "verification" and (not doc or not GRAM.fullmatch(anchor)):
            valid = False
        if not valid or anchor not in self.anchors[p]:
            self.err(f"{lab}: {ident}: ID requires explicit matching anchor")
        rel = p.relative_to(self.docs).as_posix()
        allowed = (
            (stage == "spec" and rel.startswith("specs/"))
            or (stage == "architecture" and p == self.docs / "アーキテクチャ設計書.md")
            or (stage == "design" and rel.startswith("design/"))
            or (stage == "verification" and rel.startswith("verification/cases/"))
        )
        if not allowed:
            self.err(f"{lab}: stage {stage} invalid at document location")
        if ident in self.nodes:
            self.err(f"duplicate trace node ID {ident}")
        else:
            self.nodes[ident] = Node(
                ident,
                stage,
                [C(x) for x in d["requirements"]],
                d["upstream"],
                d["state"],
                d["pending"],
                p,
            )

    def validate(self):
        for n in self.nodes.values():
            for r in n.requirements:
                if r not in self.requirements:
                    self.err(f"{n.id}: unknown requirement {r}")
            covered = set()
            for u in n.upstream:
                if u.startswith("DIR-FUNC-"):
                    if n.stage != "spec" or u not in self.functions:
                        self.err(f"{n.id}: illegal or unknown function upstream {u}")
                    else:
                        shared = {
                            r
                            for r in n.requirements
                            if u in self.req_funcs.get(r, set())
                        }
                        if not shared:
                            self.err(
                                f"{n.id}: upstream {u} has no shared requirement scope"
                            )
                        covered |= shared
                elif u.startswith("DIR-REQ-"):
                    if n.stage != "verification" or u not in self.exceptions:
                        self.err(
                            f"{n.id}: direct requirement upstream only allowed for exceptions"
                        )
                    else:
                        if u not in n.requirements:
                            self.err(
                                f"{n.id}: upstream {u} has no shared requirement scope"
                            )
                        covered.add(u)
                elif u not in self.nodes:
                    self.err(f"{n.id}: unknown upstream {u}")
                else:
                    src = self.nodes[u]
                    if NEXT.get(src.stage) != n.stage:
                        self.err(f"{n.id}: illegal stage edge {src.stage}->{n.stage}")
                    shared = set(src.requirements) & set(n.requirements)
                    if not shared:
                        self.err(
                            f"{n.id}: upstream {u} has no shared requirement scope"
                        )
                    covered |= shared
            if n.upstream:
                for r in set(n.requirements) - covered:
                    self.err(f"{n.id}: upstream allocation does not cover {r}")
        graph = {
            n.id: [u for u in n.upstream if u in self.nodes]
            for n in self.nodes.values()
        }
        vis = set()
        done = set()

        def walk(k):
            if k in vis:
                self.cyclic = True
                self.err(f"cyclic trace references include {k}")
                return
            if k in done:
                return
            vis.add(k)
            for u in graph[k]:
                walk(u)
            vis.remove(k)
            done.add(k)

        for k in graph:
            walk(k)

    def gaps(self, selected):
        out = []
        down = defaultdict(list)
        for n in self.nodes.values():
            for u in n.upstream:
                down[u].append(n)
        for r in sorted(selected):
            scoped = [n for n in self.nodes.values() if r in n.requirements]
            for n in scoped:
                if n.pending:
                    out.append(f"{r}: {n.id} has pending work")
                if not n.upstream:
                    out.append(f"{r}: {n.id} has no upstream allocation")
                if n.stage != "verification" and not [
                    x
                    for x in down[n.id]
                    if r in x.requirements and x.stage == NEXT[n.stage]
                ]:
                    out.append(f"{r}: {n.id} has no {NEXT[n.stage]} continuation")
            if r in self.exceptions:
                if not any(
                    n.stage == "verification"
                    and r in n.requirements
                    and r in n.upstream
                    for n in scoped
                ):
                    out.append(f"{r}: exception route has no verification node")
            else:
                for f in self.req_funcs.get(r, set()):
                    if not [
                        n for n in down[f] if n.stage == "spec" and r in n.requirements
                    ]:
                        out.append(f"{r}: {f} has no spec allocation")
        return sorted(set(out))

    def impact(self, raw):
        ident = (
            C(raw) if re.fullmatch(r"DIR-(?:REQ|FUNC|TEST)-\d{3}", raw, re.I) else raw
        )
        if ident not in (set(self.nodes) | self.requirements | self.functions):
            raise ValueError(f"unknown trace ID: {raw}")
        starts = (
            [(ident, ident)] + [(ident, f) for f in self.req_funcs[ident]]
            if ident in self.requirements
            else (
                [(r, ident) for r, fs in self.req_funcs.items() if ident in fs]
                if ident in self.functions
                else [(r, ident) for r in self.nodes[ident].requirements]
            )
        )
        fw = defaultdict(list)
        bw = defaultdict(list)
        for n in self.nodes.values():
            for u in n.upstream:
                fw[u].append(n)
                bw[n.id].append(u)

        def go(seed, forward):
            q = deque(seed)
            seen = set(seed)
            while q:
                r, k = q.popleft()
                targets = (
                    fw[k]
                    if forward
                    else bw[k]
                    + (
                        [r]
                        if k in self.functions and k in self.req_funcs.get(r, set())
                        else []
                    )
                )
                for t in targets:
                    nxt = t.id if forward else t
                    if isinstance(t, Node) and r not in t.requirements:
                        continue
                    if (
                        not forward
                        and isinstance(t, str)
                        and t in self.nodes
                        and r not in self.nodes[t].requirements
                    ):
                        continue
                    if (r, nxt) not in seen:
                        if (
                            not forward
                            and nxt in self.functions
                            and nxt not in self.req_funcs.get(r, set())
                        ):
                            continue
                        if not forward and nxt in self.requirements and nxt != r:
                            continue
                        seen.add((r, nxt))
                        q.append((r, nxt))
            return seen

        up, down = go(starts, False), go(starts, True)
        fmt = lambda x: ", ".join(sorted({v for _, v in x})) or "(none)"
        gaps = []
        for r, k in starts:
            if (
                r not in self.exceptions
                and k in self.functions
                and not any(n.stage == "spec" and r in n.requirements for n in fw[k])
            ):
                gaps.append(f"{r}:{k} missing spec allocation")
        for r, k in down:
            if k in self.nodes:
                n = self.nodes[k]
                if n.pending or (
                    n.stage != "verification"
                    and not any(r in x.requirements for x in fw[k])
                ):
                    gaps.append(f"{r}:{k}")
        return (
            "Impact connectivity report (requirement-scope filtered; not semantic coverage):\n"
            + f"upstream roots: {fmt(up)}\n"
            + f"downstream affected: {fmt(down)}\n"
            + f"pending gaps: {', '.join(sorted(set(gaps))) or '(none)'}"
        )


def main(argv=None):
    p = argparse.ArgumentParser()
    p.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    p.add_argument("--strict", action="store_true")
    p.add_argument("--requirement", action="append", default=[])
    p.add_argument("--impact")
    a = p.parse_args(argv)
    c = Check(a.root.resolve())
    c.sources()
    c.nodes_load()
    c.validate()
    sel = {C(x) for x in a.requirement} or c.requirements
    for r in sel - c.requirements:
        c.err(f"unknown selected requirement {r}")
    gaps = [] if c.errors or c.cyclic else c.gaps(sel & c.requirements)
    if a.impact and not c.errors:
        try:
            print(c.impact(a.impact))
        except ValueError as e:
            c.err(str(e))
    for e in c.errors:
        print(f"error: {e}", file=sys.stderr)
    for x in gaps:
        print(f"incomplete: {x}")
    incomplete_requirements = len(
        {x.split(":", 1)[0] for x in gaps if x.startswith("DIR-REQ-")}
    )
    print(
        f"traceability: {len(c.requirements)} requirements, {len(c.functions)} functions, {len(c.nodes)} nodes, {len(c.errors)} structural error(s), {incomplete_requirements} incomplete requirement(s), {len(gaps)} incomplete item(s)"
    )
    return 1 if c.errors or (a.strict and gaps) else 0


if __name__ == "__main__":
    raise SystemExit(main())
