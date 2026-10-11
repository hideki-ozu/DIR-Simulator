#!/usr/bin/env python3
"""Generate traceability reports and the requirement hierarchy HTML annex."""

from __future__ import annotations

import argparse
import html
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import quote

from check_traceability import Check, NEXT
import requirement_hierarchy


OUTPUT = Path("docs/要件トレーサビリティ一覧.md")
HTML_OUTPUT = Path("docs/要件トレーサビリティ一覧.html")
STAGES = ("spec", "architecture", "design", "verification")
STAGE_LABELS = {
    "spec": "詳細仕様",
    "architecture": "アーキテクチャ",
    "design": "詳細設計",
    "verification": "検証",
}
DOCUMENT_TITLE = re.compile(r"^#\s+(.+?)\s*$", re.M)
DOCUMENT_VERSION = re.compile(r"^文書バージョン：\s*`([^`\n]+)`\s*$", re.M)
GITHUB_VERSION = re.compile(r"^対象GitHubバージョン：\s*`([^`\n]+)`\s*$", re.M)


@dataclass
class PathRow:
    requirement: str
    kind: str
    function: str | None
    nodes: dict[str, object]
    status: str
    gaps: list[str]


def number_key(value: str) -> tuple[int, str]:
    match = re.search(r"(\d+)$", value)
    return (int(match.group(1)) if match else 0, value)


def safe_cell(value: str) -> str:
    return " ".join(value.replace("|", r"\|").splitlines()).strip()


def esc(value: object) -> str:
    return html.escape(str(value), quote=True)


def link_html(label: str, path: str, anchor: str, *, class_name: str = "") -> str:
    target = quote(path, safe="/-._~")
    href = esc(f"{target}#{anchor}")
    css = f' class="{esc(class_name)}"' if class_name else ""
    return f'<a href="{href}"{css}>{esc(label)}</a>'


def document_inventory(check: Check) -> list[tuple[str, str, str, str]]:
    paths = sorted(
        (p for p in check.docs.rglob("*.md")
         if "templates" not in p.relative_to(check.docs).parts and p != check.docs / OUTPUT.name),
        key=lambda p: p.relative_to(check.docs).as_posix(),
    )
    rows = []
    for path in paths:
        content = check.read(path)
        title_match = DOCUMENT_TITLE.search(content)
        version_match = DOCUMENT_VERSION.search(content)
        github_match = GITHUB_VERSION.search(content)
        title = safe_cell(title_match.group(1)) if title_match else path.stem
        relative = path.relative_to(check.docs).as_posix()
        rows.append(
            (
                title,
                relative,
                safe_cell(version_match.group(1)) if version_match else "未記載",
                safe_cell(github_match.group(1)) if github_match else "未記載",
            )
        )
    return rows


def _has_anchor(check: Check, path: Path, anchor: str) -> bool:
    return anchor in check.anchors.get(path, set())


def requirement_link(check: Check, requirement: str) -> str:
    path = check.docs / "要件定義書.md"
    if _has_anchor(check, path, requirement.lower()):
        return link_html(requirement, "要件定義書.md", requirement.lower(), class_name="trace-id")
    return f'<code class="trace-id">{esc(requirement)}</code>'


def function_link(check: Check, function: str) -> str:
    path = check.docs / "機能仕様書.md"
    anchor = function.lower()
    if function in check.functions and _has_anchor(check, path, anchor):
        return link_html(function, "機能仕様書.md", anchor, class_name="trace-id")
    return f'<code class="trace-id">{esc(function)}</code>'


def node_link(check: Check, node) -> str:
    rel = node.path.relative_to(check.docs).as_posix()
    anchor = node.id.lower() if node.stage == "verification" else (
        node.id.split("#", 1)[1] if "#" in node.id else ""
    )
    if anchor and _has_anchor(check, node.path, anchor):
        target = link_html(node.id, rel, anchor, class_name="trace-id")
    else:
        target = f'<code class="trace-id">{esc(node.id)}</code>'
    certainty = (
        "草案" if node.state == "draft" else
        "確定" if node.state == "confirmed" else
        f"無効状態：{node.state}"
    )
    certainty_class = "invalid-state" if node.state not in ("draft", "confirmed") else "certainty"
    return f'{target}<span class="{certainty_class}">（{esc(certainty)}）</span>'


def acceptance_link(check: Check, acceptance: str) -> str:
    path = check.docs / "要件定義書.md"
    if _has_anchor(check, path, acceptance.lower()):
        return link_html(acceptance, "要件定義書.md", acceptance.lower(), class_name="trace-id")
    return f'<code class="trace-id">{esc(acceptance)}</code>'


def affected_requirements(check: Check) -> tuple[set[str], bool]:
    requirements: set[str] = set()
    has_unscoped_error = False
    requirement_pattern = re.compile(r"DIR-REQ-\d{4}(?!\d)", re.I)
    function_pattern = re.compile(r"DIR-FUNC-\d{4}(?!\d)", re.I)
    for error in check.errors:
        mentioned = {value.upper() for value in requirement_pattern.findall(error)}
        for function in (value.upper() for value in function_pattern.findall(error)):
            mentioned.update(
                requirement
                for requirement, functions in check.req_funcs.items()
                if function in functions
            )
        if mentioned:
            requirements.update(mentioned)
        else:
            has_unscoped_error = True
    return requirements, has_unscoped_error


def _node_edges(check: Check, requirement: str, scoped: set[str]):
    """Return only real, adjacent-stage edges that share this requirement."""
    outgoing = {ident: [] for ident in scoped}
    incoming = {ident: [] for ident in scoped}
    function_starts = {function: [] for function in check.functions}
    for ident in sorted(scoped, key=number_key):
        node = check.nodes[ident]
        if node.stage != "spec":
            continue
        for upstream in node.upstream:
            if (
                upstream in check.functions
                and upstream in check.req_funcs.get(requirement, set())
            ):
                function_starts.setdefault(upstream, []).append(ident)
    for ident in sorted(scoped, key=number_key):
        node = check.nodes[ident]
        for upstream in node.upstream:
            source = check.nodes.get(upstream)
            if (
                source is None
                or upstream not in scoped
                or NEXT.get(source.stage) != node.stage
                or requirement not in source.requirements
            ):
                continue
            outgoing[upstream].append(ident)
            incoming[ident].append(upstream)
    for values in (*outgoing.values(), *incoming.values(), *function_starts.values()):
        values.sort(key=number_key)
    return outgoing, incoming, function_starts


def _follow_paths(start: str, outgoing: dict[str, list[str]]) -> list[list[str]]:
    """Enumerate every maximal simple path; stage-valid edges cannot cycle."""
    found: list[list[str]] = []

    def visit(current: str, prefix: list[str], active: set[str]):
        if current in active:
            # Invalid stage cycles are diagnosed by Check and never traversed as
            # valid edges, but keep this guard for damaged or future metadata.
            found.append(prefix)
            return
        active.add(current)
        children = [child for child in outgoing.get(current, []) if child not in active]
        if not children:
            found.append(prefix)
        else:
            for child in children:
                visit(child, prefix + [child], active.copy())

    visit(start, [start], set())
    return found


def _path_node_map(check: Check, path: list[str]) -> dict[str, object]:
    return {check.nodes[ident].stage: check.nodes[ident] for ident in path}


def _path_gaps(check: Check, path: list[str], *, detached=False, exception=False) -> list[str]:
    if exception:
        return [
            "保留作業あり（詳細は検証ケース本文を参照）"
            for ident in path
            if check.nodes[ident].pending
        ]
    labels: list[str] = []
    if detached:
        labels.append("機能起点の経路に接続していません")
    if path:
        first_stage = check.nodes[path[0]].stage
        first_index = STAGES.index(first_stage)
        if first_index:
            labels.extend(f"{STAGE_LABELS[stage]}：上流未接続" for stage in STAGES[:first_index])
        last_stage = check.nodes[path[-1]].stage
        last_index = STAGES.index(last_stage)
        if last_index < len(STAGES) - 1:
            labels.append(f"{STAGE_LABELS[STAGES[last_index + 1]]}：下流未割当")
            labels.extend(f"{STAGE_LABELS[stage]}：未到達" for stage in STAGES[last_index + 2 :])
    for ident in path:
        pending = check.nodes[ident].pending
        if pending:
            labels.append(f"{STAGE_LABELS[check.nodes[ident].stage]}：保留作業あり（詳細は該当文書を参照）")
    return labels


def _route_rows(check: Check):
    actual: dict[str, list[PathRow]] = {}
    detached: dict[str, list[PathRow]] = {}
    exceptions: dict[str, list[PathRow]] = {}
    for requirement in sorted(check.requirements, key=number_key):
        scoped = {
            ident
            for ident, node in check.nodes.items()
            if requirement in node.requirements
        }
        outgoing, incoming, function_starts = _node_edges(check, requirement, scoped)
        if requirement in check.exceptions:
            rows = []
            verify_nodes = sorted(
                (
                    ident
                    for ident in scoped
                    if check.nodes[ident].stage == "verification"
                    and requirement in check.nodes[ident].upstream
                ),
                key=number_key,
            )
            for ident in verify_nodes:
                path = [ident]
                pending = bool(check.nodes[ident].pending)
                rows.append(
                    PathRow(
                        requirement,
                        "exception",
                        None,
                        _path_node_map(check, path),
                        "例外経路・保留あり" if pending else "例外経路・完了",
                        _path_gaps(check, path, exception=True),
                    )
                )
            if not rows:
                rows.append(
                    PathRow(
                        requirement,
                        "exception",
                        None,
                        {},
                        "例外経路・検証未割当",
                        ["検証：下流未割当"],
                    )
                )
            exceptions[requirement] = rows
            optional_rows = []
            optional_reachable: set[str] = set()
            functions = sorted(
                check.req_funcs.get(requirement, set()) & check.functions,
                key=number_key,
            )
            for function in functions:
                for start in function_starts.get(function, []):
                    for path in _follow_paths(start, outgoing):
                        optional_reachable.update(path)
                        pending = any(check.nodes[item].pending for item in path)
                        reference_gaps = _path_gaps(check, path)
                        optional_rows.append(
                            PathRow(
                                requirement,
                                "applicability",
                                function,
                                _path_node_map(check, path),
                                "適用先の参考経路" + ("・保留ノードあり" if pending else ""),
                                reference_gaps,
                            )
                        )
            if optional_rows:
                actual.setdefault(requirement, []).extend(optional_rows)
            # Other scoped nodes stay visible as detached fragments. A valid
            # stage chain that reaches one of the direct exception tests is
            # retained as a detached incoming route as well.
            detached_nodes = scoped - set(verify_nodes) - optional_reachable
            roots = sorted(
                (ident for ident in detached_nodes if not (set(incoming[ident]) & detached_nodes)),
                key=number_key,
            )
            for ident in roots:
                for path in _follow_paths(ident, outgoing):
                    detached.setdefault(requirement, []).append(
                        PathRow(
                            requirement,
                            "detached",
                            None,
                            _path_node_map(check, path),
                            "未接続（宣言対象）",
                            _path_gaps(check, path, detached=True),
                        )
                    )
        else:
            rows = []
            reachable: set[str] = set()
            functions = sorted(
                check.req_funcs.get(requirement, set()) & check.functions,
                key=number_key,
            )
            for function in functions:
                starts = function_starts.get(function, [])
                if not starts:
                    rows.append(
                        PathRow(
                            requirement,
                            "connected",
                            function,
                            {},
                            "詳細仕様への割当不足",
                            ["詳細仕様：下流未割当", "後続工程：未到達"],
                        )
                    )
                    continue
                for start in starts:
                    for path in _follow_paths(start, outgoing):
                        reachable.update(path)
                        nodes = _path_node_map(check, path)
                        pending = any(check.nodes[ident].pending for ident in path)
                        complete = check.nodes[path[-1]].stage == "verification"
                        status = (
                            "経路完備・保留あり"
                            if complete and pending
                            else "経路完備"
                            if complete
                            else "追跡中・保留あり"
                            if pending
                            else "追跡中"
                        )
                        rows.append(
                            PathRow(
                                requirement,
                                "connected",
                                function,
                                nodes,
                                status,
                                _path_gaps(check, path),
                            )
                        )
            if not functions:
                rows.append(
                    PathRow(
                        requirement,
                        "connected",
                        None,
                        {},
                        "有効な機能割当なし",
                        ["機能：有効な割当なし", "詳細仕様以降：未到達"],
                    )
                )
            if rows:
                actual[requirement] = rows

            # Detached roots are traversed independently even when a fragment
            # later reconverges with an already reachable node. This preserves
            # every valid scoped incoming edge without calling it function-led.
            detached_nodes = scoped - reachable
            roots = sorted(
                (ident for ident in detached_nodes if not (set(incoming[ident]) & detached_nodes)),
                key=number_key,
            )
            for ident in sorted(roots, key=number_key):
                for path in _follow_paths(ident, outgoing):
                    path_nodes = _path_node_map(check, path)
                    last_stage = check.nodes[path[-1]].stage
                    missing = _path_gaps(check, path, detached=True)
                    status = (
                        "未接続（宣言対象）・下流未完了"
                        if last_stage != "verification"
                        else "未接続（宣言対象）"
                    )
                    if any(check.nodes[item].pending for item in path):
                        status += "・保留あり"
                    detached.setdefault(requirement, []).append(
                        PathRow(requirement, "detached", None, path_nodes, status, missing)
                    )
            # Defensive fallback for malformed cyclic metadata. Valid adjacent
            # stage edges are acyclic, but this ensures each registered node is
            # still visible if a future stage model admits a cycle.
            seen_detached = {
                ident
                for row in detached.get(requirement, [])
                for ident in row.nodes.values()
                for ident in [ident.id]
            }
            for ident in sorted(detached_nodes - seen_detached, key=number_key):
                path = [ident]
                detached.setdefault(requirement, []).append(
                    PathRow(
                        requirement,
                        "detached",
                        None,
                        _path_node_map(check, path),
                        "未接続（宣言対象・巡回参照あり）",
                        _path_gaps(check, path, detached=True),
                    )
                )

    for groups in (actual, detached, exceptions):
        for rows in groups.values():
            rows.sort(key=lambda row: (number_key(row.function or ""), tuple(
                number_key(row.nodes[stage].id) if stage in row.nodes else (0, "")
                for stage in STAGES
            )))
    return actual, detached, exceptions


def _stage_cells(check: Check, row: PathRow) -> dict[str, str]:
    cells = {}
    if row.kind == "exception":
        for stage in STAGES[:-1]:
            cells[stage] = "<span class=\"not-applicable\">非該当（例外）</span>"
        node = row.nodes.get("verification")
        cells["verification"] = node_link(check, node) if node else (
            '<span class="missing">下流未割当</span>'
        )
        return cells
    present = [index for index, stage in enumerate(STAGES) if stage in row.nodes]
    if not present:
        if row.function:
            cells[STAGES[0]] = '<span class="missing">下流未割当</span>'
            for stage in STAGES[1:]:
                cells[stage] = '<span class="unreached">未到達</span>'
        else:
            for stage in STAGES:
                cells[stage] = '<span class="unallocated">上流未割当</span>' if stage == STAGES[0] else '<span class="unreached">未到達</span>'
        return cells
    first, last = min(present), max(present)
    for index, stage in enumerate(STAGES):
        node = row.nodes.get(stage)
        if node:
            cells[stage] = node_link(check, node)
        elif index < first:
            cells[stage] = '<span class="unallocated">上流未接続</span>'
        elif index == last + 1:
            cells[stage] = '<span class="missing">下流未割当</span>'
        elif index > last:
            cells[stage] = '<span class="unreached">未到達</span>'
        else:
            cells[stage] = '<span class="unreached">経路上未到達</span>'
    return cells


def _merge_keys(row: PathRow, cells: dict[str, str]) -> list[tuple]:
    row_kind = row.kind
    req_prefix = (row.requirement, row_kind)
    function_prefix = req_prefix + (row.function or "<no-function>",)
    keys = [req_prefix, function_prefix]
    prefix = function_prefix
    for stage in STAGES:
        node = row.nodes.get(stage)
        identity = node.id if node else f"<placeholder:{re.sub('<[^>]+>', '', cells[stage])}>"
        prefix = prefix + (identity,)
        keys.append(prefix)
    keys.extend((None, None))
    return keys


def _table_group(rows: list[PathRow], check: Check, *, kind: str, label: str) -> str:
    if not rows:
        return ""
    requirement = rows[0].requirement
    parts = [f'<tbody class="group-{esc(kind)}" data-requirement="{esc(requirement)}">', f'<tr class="group-label"><th colspan="8">{esc(label)}</th></tr>']
    rendered = []
    affected, unscoped = affected_requirements(check)
    for row in rows:
        cells = _stage_cells(check, row)
        req_title = check.req_titles.get(row.requirement, "要件名未登録")
        req_value = f'{requirement_link(check, row.requirement)}<br><span class="req-title">{esc(req_title)}</span>'
        if row.kind == "exception":
            func_value = '<span class="not-applicable">—（例外経路）</span>'
        elif row.function:
            func_value = function_link(check, row.function)
        else:
            func_value = '<span class="unallocated">上流未接続</span>' if row.kind == "detached" else '<span class="unallocated">上流未割当</span>'
        gap_value = "<br>".join(esc(gap) for gap in row.gaps) if row.gaps else "—"
        needs_correction = unscoped or row.requirement in affected
        visible_status = f"🔴 要修正：{row.status}" if needs_correction else row.status
        status_class = (
            "error" if needs_correction
            else "incomplete" if row.kind == "applicability" and row.gaps
            else "reference" if row.kind == "applicability"
            else "incomplete" if row.gaps
            else "complete"
        )
        rendered.append(
            {
                "values": [
                    req_value,
                    func_value,
                    *(cells[stage] for stage in STAGES),
                    f'<span class="status {status_class}">{esc(visible_status)}</span>',
                    gap_value,
                ],
                "keys": _merge_keys(row, cells),
            }
        )
    mergeable = range(6)
    i = 0
    while i < len(rendered):
        parts.append("<tr>")
        for col in range(8):
            if col in mergeable:
                key = rendered[i]["keys"][col]
                if i and rendered[i - 1]["keys"][col] == key:
                    continue
                span = 1
                while i + span < len(rendered) and rendered[i + span]["keys"][col] == key:
                    span += 1
                content = rendered[i]["values"][col]
                attrs = f' rowspan="{span}"' if span > 1 else ""
                cls = f' class="{("stage-" + STAGES[col - 2]) if 2 <= col <= 5 else ("req-cell" if col == 0 else "function-cell")}"'
                parts.append(f"<td{attrs}{cls}>{content}</td>")
            else:
                parts.append(f'<td class="{"status-cell" if col == 6 else "gap-cell"}">{rendered[i]["values"][col]}</td>')
        parts.append("</tr>")
        i += 1
    parts.append("</tbody>")
    return "\n".join(parts)


def _path_section(check: Check, groups, requirement: str, summary_status: str) -> str:
    actual, detached, exceptions = groups
    req_rows = []
    if requirement in actual:
        connected = [row for row in actual[requirement] if row.kind == "connected"]
        applicability = [row for row in actual[requirement] if row.kind == "applicability"]
        if connected:
            req_rows.append(_table_group(connected, check, kind="connected", label="機能割当から実際の接続をたどった経路"))
        if applicability:
            req_rows.append(_table_group(applicability, check, kind="applicability", label="機能の適用先として記録された参考経路（例外経路の必須工程ではありません）"))
    if requirement in detached:
        req_rows.append(_table_group(detached[requirement], check, kind="detached", label="宣言対象のノード群（機能起点の実接続経路ではありません）"))
    if requirement in exceptions:
        req_rows.append(_table_group(exceptions[requirement], check, kind="exception", label="規約に登録された要件から検証への例外経路"))
    if not req_rows:
        return ""
    return (
        f'<section class="requirement-block" data-requirement="{esc(requirement)}"><h3>{requirement_link(check, requirement)} '
        f'<span class="req-title">{esc(check.req_titles.get(requirement, "要件名未登録"))}</span> '
        f'<span class="status-badge">要件状態：{esc(summary_status)}</span></h3>\n'
        '<div class="table-scroll"><table class="trace-table"><thead><tr>'
        '<th scope="col">要件</th><th scope="col">機能</th>'
        + "".join(f'<th scope="col" class="stage-{stage}">{STAGE_LABELS[stage]}</th>' for stage in STAGES)
        + '<th scope="col">経路の状態</th><th scope="col">未完了・接続上の説明</th>'
        '</tr></thead>\n'
        + "\n".join(req_rows)
        + "</table></div></section>"
    )


def _summary(check: Check, all_gaps: list[str]):
    error_requirements, has_unscoped_error = affected_requirements(check)
    totals = {"complete": 0, "partial": 0, "exception": 0, "error": 0}
    statuses = {}
    for requirement in sorted(check.requirements, key=number_key):
        requirement_gaps = [gap for gap in all_gaps if gap.startswith(f"{requirement}: ")]
        if has_unscoped_error or requirement in error_requirements:
            status = "🔴 要修正"
            totals["error"] += 1
        elif requirement in check.exceptions:
            if requirement_gaps:
                status = "🟡 例外経路・未完了"
                totals["partial"] += 1
            else:
                status = "🔵 例外経路完了"
                totals["exception"] += 1
        elif requirement_gaps:
            status = "🟡 追跡中"
            totals["partial"] += 1
        else:
            status = "🟢 経路完備"
            totals["complete"] += 1
        statuses[requirement] = status
    return totals, statuses


def _inventory_table(check: Check) -> str:
    rows = []
    for title, relative, version, github in document_inventory(check):
        rows.append(
            "<tr><td>"
            + f'<a href="{esc(quote(relative, safe="/-._~"))}">{esc(title)}</a></td>'
            + f"<td>{esc(version)}</td><td>{esc(github)}</td></tr>"
        )
    return (
        '<div class="table-scroll"><table><thead><tr><th scope="col">文書名</th>'
        '<th scope="col">文書バージョン</th><th scope="col">GitHub対象バージョン</th>'
            '</tr></thead><tbody>' + "\n".join(rows) + "</tbody></table></div>"
    )


def _acceptance_table(check: Check) -> str:
    rows = []
    for requirement in sorted(check.requirements, key=number_key):
        acceptance = sorted(check.req_acs.get(requirement, set()), key=number_key)
        values = acceptance or ["—"]
        for item in values:
            rows.append(
                (
                    requirement,
                    requirement_link(check, requirement),
                    esc(check.req_titles.get(requirement, "要件名未登録")),
                    acceptance_link(check, item) if item != "—" else "—",
                    item,
                )
            )
    return _simple_table(
        ["要件ID", "要件概要", "受入条件ID"],
        [(req_cell, title, ac) for _, req_cell, title, ac, _ in rows],
        rowspan_columns={0},
        row_keys=[(req, ac_id) for req, _, _, _, ac_id in rows],
        classes=["", "", ""],
    )


def _simple_table(headers, rows, *, rowspan_columns=frozenset(), row_keys=None, classes=None):
    if row_keys is None:
        row_keys = [(i,) for i in range(len(rows))]
    parts = ['<div class="table-scroll"><table><thead><tr>']
    parts.extend(f'<th scope="col">{esc(header)}</th>' for header in headers)
    parts.append("</tr></thead><tbody>")
    for i, row in enumerate(rows):
        parts.append("<tr>")
        for col, value in enumerate(row):
            if col in rowspan_columns:
                span = 1
                key = row_keys[i][0]
                while i + span < len(rows) and row_keys[i + span][0] == key:
                    span += 1
                # Render each merged cell at its first row only. The caller's
                # keys are sorted by requirement, so these spans remain local.
                if i and row_keys[i - 1][0] == key:
                    continue
                attrs = f' rowspan="{span}"' if span > 1 else ""
            else:
                attrs = ""
            klass = f' class="{esc(classes[col])}"' if classes and classes[col] else ""
            parts.append(f"<td{attrs}{klass}>{value}</td>")
        parts.append("</tr>")
    parts.append("</tbody></table></div>")
    return "\n".join(parts)


def _exception_applicability_table(check: Check) -> str:
    rows = []
    for requirement in sorted(check.exceptions, key=number_key):
        functions = sorted(check.req_funcs.get(requirement, set()), key=number_key)
        if not functions:
            rows.append((requirement, requirement_link(check, requirement), esc(check.req_titles.get(requirement, "要件名未登録")), "—（適用先未登録）"))
        for function in functions:
            value = function_link(check, function) if function in check.functions else f'<code>{esc(function)}</code>'
            rows.append((requirement, requirement_link(check, requirement), esc(check.req_titles.get(requirement, "要件名未登録")), value))
    return _simple_table(
        ["例外要件ID", "要件概要", "適用先（機能）"],
        [(req, title, function) for _, req, title, function in rows],
        rowspan_columns={0},
        row_keys=[(requirement, function) for requirement, _, _, function in rows],
    )


def _unassigned_table(check: Check) -> str:
    rows = []
    assigned_functions = {
        function
        for requirement, functions in check.req_funcs.items()
        if requirement in check.requirements
        for function in functions
        if function in check.functions
    }
    for function in sorted(check.functions - assigned_functions, key=number_key):
        rows.append(("機能", function_link(check, function), "§11.2の有効な要件割当がありません"))
    for ident, node in sorted(check.nodes.items(), key=lambda item: number_key(item[0])):
        valid_scopes = set(node.requirements) & check.requirements
        invalid_scopes = sorted(set(node.requirements) - check.requirements, key=number_key)
        if not valid_scopes or invalid_scopes:
            if invalid_scopes:
                for requirement in invalid_scopes:
                    rows.append(("トレースノード", node_link(check, node), f"未知または無効な要件スコープ：{requirement}"))
            else:
                rows.append(("トレースノード", node_link(check, node), "有効要件に属するスコープがありません"))
    if not rows:
        rows.append(("—", "—", "該当項目なし"))
    return _simple_table(["種別", "ID", "未割当・無効スコープ"], rows)


def report_body(check: Check) -> str:
    all_gaps = check.gaps(check.requirements)
    totals, statuses = _summary(check, all_gaps)
    route_groups = _route_rows(check)
    summary_rows = [
        ("有効要件", str(len(check.requirements))),
        ("🔴 要修正の要件", str(totals["error"])),
        ("構造エラー", str(len(check.errors))),
        ("🟢 経路完備", str(totals["complete"])),
        ("🟡 追跡中・未完了", str(totals["partial"])),
        ("🔵 完了した例外経路", str(totals["exception"])),
        ("未完了項目", str(len(all_gaps))),
    ]
    requirements_html = []
    for requirement in sorted(check.requirements, key=number_key):
        section = _path_section(check, route_groups, requirement, statuses.get(requirement, "状態未判定"))
        if section:
            requirements_html.append(section)
    errors = list(check.errors)
    diagnostic_rows = "".join(f"<li><code>{esc(error)}</code></li>" for error in errors)
    body = [
        '<section id="inventory"><h2>文書と対象バージョン</h2>',
        '<p>文書名・文書バージョン・GitHub対象バージョンを各文書の冒頭から転記しています。テンプレートとこの自動生成一覧は除外し、値が冒頭にない場合は「未記載」と表示します。</p>',
        _inventory_table(check),
        '</section><section id="summary"><h2>集計</h2>',
        _simple_table(["集計", "件数"], summary_rows),
        '<p class="legend">凡例：🟢 必要な工程間の接続あり、🔵 規約に登録された例外経路、🟡 未作成・保留・未接続あり、🔴 文書・ID・工程間の構造エラーあり。記号と状態名を併記しています。</p>',
        '<p>経路は要件と実際の隣接工程間の <code>trace.upstream</code> 接続に沿って作成しています。追跡経路は仕様内容の妥当性や検証合格を示すものではありません。</p>',
        '</section><section id="routes"><h2>要件から検証までの経路</h2>',
        '<p>各行は記録された接続を末端までたどった完了または未完了の経路です。同じIDが複数要件に現れる場合も要件ごとに独立したセルで表示します。色に加えて状態名を表示しています。</p>',
        *requirements_html,
        '</section><section id="acceptance"><h2>要件と受入条件</h2>',
        '<p>要件定義書に登録された受入条件だけを表示します。機能から受入条件への接続は仮定していません。</p>',
        _acceptance_table(check),
        '</section>',
    ]
    if check.exceptions:
        body.extend(
            [
                '<section id="exception-applicability"><h2>例外要件の適用先</h2>',
                '<p>例外経路に割り当てられた機能は適用先として示し、要件から検証までの必須工程経路には含めません。</p>',
                _exception_applicability_table(check),
                '</section>',
            ]
        )
    body.extend(
        [
            '<section id="unassigned"><h2>未割当・無効スコープ</h2>',
            '<p>未割当の機能、または有効要件に接続できないトレースノードを示します。無効な参照先はリンクを作らず、構造診断に記載します。</p>',
            _unassigned_table(check),
            '</section>',
        ]
    )
    if errors:
        body.extend(
            [
                '<section id="diagnostics" class="diagnostics"><h2>構造エラーと参照診断</h2>',
                '<p>構造エラーがある場合も説明用レポートを生成します。生成コマンドはエラー終了します。</p>',
                f'<ul>{diagnostic_rows}</ul></section>',
            ]
        )
    return "\n".join(body)


CSS = """\
:root { color-scheme: light; font-family: system-ui, -apple-system, "Noto Sans JP", sans-serif; color: #1f2937; background: #fff; }
body { margin: 0 auto; padding: 1.5rem; max-width: 1600px; line-height: 1.55; }
h1, h2, h3 { line-height: 1.25; }
h1 { margin: 0 0 .5rem; }
h2 { margin-top: 2.4rem; padding-bottom: .35rem; border-bottom: 2px solid #cbd5e1; }
h3 { margin: 1.8rem 0 .65rem; font-size: 1.1rem; }
p { max-width: 100ch; }
.report-link { padding: .8rem 1rem; background: #eff6ff; border-left: 4px solid #60a5fa; }
.table-scroll { overflow-x: auto; margin: .7rem 0 1.2rem; }
table { border-collapse: collapse; width: 100%; min-width: 900px; background: #fff; }
th, td { border: 1px solid #94a3b8; padding: .45rem .55rem; text-align: left; vertical-align: top; overflow-wrap: anywhere; }
thead th { position: sticky; top: 0; z-index: 2; background: #e2e8f0; }
th { font-weight: 650; }
td { min-width: 7rem; }
.trace-table { min-width: 1450px; table-layout: fixed; }
.trace-table th:nth-child(1) { width: 13rem; }
.trace-table th:nth-child(2) { width: 11rem; }
.trace-table th:nth-child(n+3):nth-child(-n+6) { width: 12rem; }
.trace-table th:nth-child(7) { width: 12rem; }
.trace-table th:nth-child(8) { width: 20rem; }
.stage-spec { background: #eff6ff; }
.stage-architecture { background: #ecfeff; }
.stage-design { background: #f0fdf4; }
.stage-verification { background: #f5f3ff; }
tbody.group-connected .stage-spec, tbody.group-connected .stage-architecture, tbody.group-connected .stage-design, tbody.group-connected .stage-verification { background: inherit; }
tbody.group-connected td.stage-spec { background: #eff6ff; }
tbody.group-connected td.stage-architecture { background: #ecfeff; }
tbody.group-connected td.stage-design { background: #f0fdf4; }
tbody.group-connected td.stage-verification { background: #f5f3ff; }
tbody.group-detached td { background-color: #f8fafc; }
tbody.group-detached td.stage-spec { background: #eff6ff; }
tbody.group-detached td.stage-architecture { background: #ecfeff; }
tbody.group-detached td.stage-design { background: #f0fdf4; }
tbody.group-detached td.stage-verification { background: #f5f3ff; }
tbody.group-exception td { background-color: #f8fafc; }
.group-label th { background: #e2e8f0; border-top: 2px solid #64748b; }
.trace-id { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; white-space: normal; overflow-wrap: anywhere; }
.req-title, .certainty { color: #475569; font-size: .92em; }
td.req-cell { background: #f8fafc; }
td.function-cell { background: #fff7ed; }
.missing, .unallocated { display: inline-block; padding: .08rem .25rem; background: #fff7d6; border-radius: .2rem; }
.unreached { color: #64748b; }
.not-applicable { color: #475569; font-style: italic; }
.status { font-weight: 650; }
.status.incomplete { color: #854d0e; }
.status.complete { color: #166534; }
.status.reference { color: #1d4ed8; }
.status.error, .diagnostics { color: #9f1239; }
.diagnostics { padding: .4rem .8rem; background: #fff1f2; border: 1px solid #fda4af; }
.legend { padding: .7rem; background: #f8fafc; }
a { color: #1d4ed8; }
@media (max-width: 700px) { body { padding: .8rem; } }
"""


def render(check: Check) -> str:
    body = report_body(check)
    return "\n".join(
        [
            "<!-- Generated by scripts/generate_traceability.py. Do not edit manually. -->",
            "# 要件トレーサビリティ一覧",
            "",
            '<p class="report-link"><strong>見やすい横長レポート：</strong><a href="要件トレーサビリティ一覧.html">要件トレーサビリティ一覧.html を開く</a>（色分けと横スクロールに対応）</p>',
            "",
            "この一覧は要件定義書・機能仕様書・各 `trace` 記録から自動生成しています。対応関係の正本は各仕様・設計文書です。",
            "",
            body,
            "",
        ]
    )


def render_html(check: Check) -> str:
    body = report_body(check)
    return "\n".join(
        [
        "<!doctype html>",
        "<!-- Generated by scripts/generate_traceability.py. Do not edit manually. -->",
        '<html lang="ja">',
            "<head>",
            '<meta charset="utf-8">',
            '<meta name="viewport" content="width=device-width, initial-scale=1">',
            "<title>要件トレーサビリティ一覧</title>",
            f"<style>\n{CSS}\n</style>",
            "</head>",
            "<body>",
            '<header><h1>要件トレーサビリティ一覧</h1><p>要件と各工程の実際の接続経路を示します。検証ケースの実行結果は各ケース本文を確認してください。</p></header>',
            body,
            "</body>",
            "</html>",
            "",
        ]
    )


def _is_current(path: Path, rendered: str) -> bool:
    try:
        return path.is_file() and path.read_text(encoding="utf-8") == rendered
    except (OSError, UnicodeError):
        return False


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository snapshot to read and write (default: this checkout)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if any generated report or hierarchy annex is missing or out of date",
    )
    args = parser.parse_args(argv)

    root = args.root.resolve()
    check = Check(root)
    check.sources()
    check.nodes_load()
    check.validate()
    req_anchors = check.anchors.get(check.docs / "要件定義書.md", set())
    for requirement, acceptance_ids in sorted(check.req_acs.items()):
        for acceptance in sorted(acceptance_ids, key=number_key):
            if acceptance.lower() not in req_anchors:
                check.err(f"{requirement}: unknown acceptance-condition anchor {acceptance}")
    try:
        hierarchy = requirement_hierarchy.load(check)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    reports = {
        OUTPUT: render(check),
        HTML_OUTPUT: render_html(check),
        requirement_hierarchy.OUTPUT: requirement_hierarchy.render(check, hierarchy, CSS),
    }
    if args.check:
        stale = [output for output, content in reports.items()
                 if not _is_current(root / output, content)]
        for output in stale:
            print(
                f"{output.as_posix()} is stale; run python3 scripts/generate_traceability.py",
                file=sys.stderr,
            )
        if check.errors:
            for error in check.errors:
                print(f"error: {error}", file=sys.stderr)
        if stale or check.errors:
            return 1
        print("All traceability reports and the hierarchy annex are up to date")
        return 0

    try:
        for output, content in reports.items():
            path = root / output
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
    except OSError as error:
        print(f"cannot write generated reports: {error}", file=sys.stderr)
        return 1
    print(
        f"generated traceability Markdown/HTML and hierarchy HTML: "
        f"{len(check.requirements)} requirements, {len(check.nodes)} trace nodes, "
        f"{len(check.gaps(check.requirements))} incomplete items"
    )
    if check.errors:
        for error in check.errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
