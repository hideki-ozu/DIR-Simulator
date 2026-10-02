/* Offline result viewer. Input strings are inserted only with textContent. */
"use strict";
(() => {
  const API = window.DIRViewerModel;
  const $ = id => document.getElementById(id);
  const SVG_NS = "http://www.w3.org/2000/svg";
  const PAGE_SIZE = 75;
  const DRAW_LIMIT = 500;
  const LABELS = {
    not_generated: "生成前", processing: "TX 処理中", pending: "送信待機",
    in_flight: "送信中", success: "送信成功", dropped: "破棄",
    not_created: "通知前", received: "受信完了", filtered: "フィルタ拒否",
    idle: "アイドル", transmitting: "送信中", intermission: "バス間隔"
  };
  const COLORS = { processing: "#88a4c8", pending: "#d3b269", in_flight: "#187f88", intermission: "#b8c4d1", received: "#5aaf95", dropped: "#cf7180" };
  let model = null;
  let chronologicalRequests = [];
  let current = 0n;
  let viewStart = 0n;
  let viewEnd = 0n;
  let selected = null;
  let page = 0;
  let playing = false;
  let animation = 0;
  let playAnchor = 0n;
  let playWallTime = 0;
  let loadVersion = 0;
  let dragDepth = 0;
  let cursorLine = null;
  let plotGeometry = null;
  let lastRendered = 0;

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = String(text);
    return node;
  }
  function svg(tag, attrs, text) {
    const node = document.createElementNS(SVG_NS, tag);
    for (const [key, value] of Object.entries(attrs || {})) node.setAttribute(key, String(value));
    if (text !== undefined) node.textContent = String(text);
    return node;
  }
  function statusBadge(state, receiver = false) {
    const label = receiver && state === "pending" ? "受信待機" : LABELS[state] || state;
    return element("span", `status ${state}`, label);
  }
  function unit() { return $("time-unit").value; }
  function unitLabel() { return unit() === "us" ? "µs" : unit(); }
  function time(value, withUnit = false) {
    if (value === null || value === undefined) return "—";
    return API.formatTime(value, unit()) + (withUnit ? ` ${unitLabel()}` : "");
  }
  function exact(value) { return value === null || value === undefined ? "未確定" : value.toString(); }
  function clampTime(value) { return value < model.start ? model.start : value > model.end ? model.end : value; }
  function enableControls(enabled) {
    document.querySelectorAll("[data-requires-model]").forEach(control => { control.disabled = !enabled; });
  }
  function pause() {
    playing = false;
    cancelAnimationFrame(animation);
    animation = 0;
    $("play").textContent = "▶ 再生";
    $("play").setAttribute("aria-label", "シミュレーションを再生");
    $("play").setAttribute("aria-pressed", "false");
  }
  function clearResult() {
    pause();
    model = null;
    chronologicalRequests = [];
    selected = null;
    current = viewStart = viewEnd = 0n;
    page = 0;
    cursorLine = null;
    plotGeometry = null;
    $("dashboard").hidden = true;
    $("empty-state").hidden = false;
    $("request-rows").replaceChildren();
    $("timeline").replaceChildren();
    $("network").replaceChildren();
    $("node-states").replaceChildren();
    $("bus-states").replaceChildren();
    $("detail-raw").textContent = "";
    $("detail-content").hidden = true;
    $("detail-empty").hidden = false;
    $("clear-selection").hidden = true;
    $("jump-error").hidden = true;
    $("request-search").value = "";
    $("status-filter").value = "all";
    enableControls(false);
  }
  function failLoad(message) {
    clearResult();
    $("loading").hidden = true;
    $("error-banner").textContent = `結果を読み込めませんでした。\n${message}`;
    $("error-banner").hidden = false;
  }
  async function loadFile(file) {
    const version = ++loadVersion;
    clearResult();
    $("error-banner").hidden = true;
    $("loading").hidden = false;
    if (!file) { $("loading").hidden = true; return; }
    try {
      const text = await file.text();
      if (version !== loadVersion) return;
      loadObject(JSON.parse(text.replace(/^\uFEFF/, "")), file.name);
    } catch (error) {
      if (version === loadVersion) failLoad(error instanceof Error ? error.message : String(error));
    }
  }
  function loadObject(raw, filename) {
    if (!API) throw new Error("model.js を読み込めません。ビューアーのファイル一式を同じフォルダーに置いてください。");
    model = API.parseResults(raw);
    chronologicalRequests = [...model.requests].sort((a, b) => {
      if (a.generated !== b.generated) return a.generated < b.generated ? -1 : 1;
      return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
    current = model.start;
    viewStart = model.start;
    viewEnd = model.end;
    selected = null;
    page = 0;
    const simulation = raw.simulation || {};
    const termination = {
      events_exhausted: "イベント完了", time_limit: "時間上限で終了", execution_failed: "実行失敗", prep_failed: "準備失敗",
      event_limit: "イベント上限", delta_cycle_limit: "デルタサイクル上限"
    }[simulation.termination] || simulation.termination || "終了理由なし";
    $("filename").textContent = filename;
    $("filename").title = filename;
    $("run-id").textContent = `RUN ${raw.run_id || "—"}`;
    $("run-id").title = raw.run_id || "";
    $("termination-badge").textContent = termination;
    $("termination-badge").className = `badge${simulation.termination === "execution_failed" ? " warning" : ""}`;
    $("partial-badge").hidden = !simulation.partial;
    $("empty-state").hidden = true;
    $("dashboard").hidden = false;
    $("error-banner").hidden = true;
    $("loading").hidden = true;
    enableControls(true);
    renderTimeline();
    renderCurrent();
    $("announcement").textContent = `${filename} を読み込みました。要求 ${model.requests.length} 件、ノード ${model.nodes.length} 件。`;
  }
  function lowerBound(times, target) {
    let left = 0, right = times.length;
    while (left < right) {
      const middle = Math.floor((left + right) / 2);
      if (times[middle] < target) left = middle + 1;
      else right = middle;
    }
    return left;
  }
  function eventStep(direction) {
    if (!model) return;
    pause();
    const times = model.eventTimes;
    let index = lowerBound(times, current);
    if (direction > 0 && times[index] === current) index++;
    if (direction < 0) index--;
    if (index >= 0 && index < times.length) setCurrent(times[index], true);
  }
  function updateNavigation() {
    const times = model.eventTimes;
    const index = lowerBound(times, current);
    $("prev-time").disabled = index === 0;
    $("next-time").disabled = index >= times.length || (index === times.length - 1 && times[index] === current);
    $("play").disabled = model.start === model.end;
    $("time-slider").disabled = model.start === model.end;
    $("zoom-in").disabled = viewEnd - viewStart <= 1n;
    $("zoom-out").disabled = viewStart === model.start && viewEnd === model.end;
    $("zoom-reset").disabled = $("zoom-out").disabled;
  }
  function setCurrent(value, scrollViewport = false) {
    if (!model) return;
    current = clampTime(value);
    if (scrollViewport && (current < viewStart || current > viewEnd)) {
      setViewport(viewEnd - viewStart, current);
      renderTimeline();
    }
    renderCurrent();
  }
  function startPlayback() {
    if (!model || model.start === model.end || $("play").disabled) return;
    if (current >= model.end) setCurrent(model.start);
    playing = true;
    playAnchor = current;
    playWallTime = performance.now();
    lastRendered = 0;
    $("play").textContent = "Ⅱ 一時停止";
    $("play").setAttribute("aria-label", "再生を一時停止");
    $("play").setAttribute("aria-pressed", "true");
    animation = requestAnimationFrame(playTick);
  }
  function playTick(wallTime) {
    if (!playing || !model) return;
    const elapsed = BigInt(Math.floor(Math.max(0, wallTime - playWallTime) * 1000 * Number($("speed").value)));
    const next = playAnchor + (model.end - model.start) * elapsed / 8000000n;
    if (next >= model.end) {
      setCurrent(model.end, true);
      pause();
      $("announcement").textContent = "観測終端に到達しました。";
      return;
    }
    if (wallTime - lastRendered >= 32) {
      setCurrent(next, true);
      lastRendered = wallTime;
    }
    animation = requestAnimationFrame(playTick);
  }
  function setViewport(span, center = current) {
    const total = model.end - model.start;
    if (span >= total) { viewStart = model.start; viewEnd = model.end; return; }
    if (span < 1n) span = 1n;
    let start = center - span / 2n;
    if (start < model.start) start = model.start;
    if (start + span > model.end) start = model.end - span;
    viewStart = start;
    viewEnd = start + span;
  }
  function zoom(factor) {
    if (!model) return;
    const span = viewEnd - viewStart;
    setViewport(factor > 1 ? span * 2n : span / 2n);
    renderTimeline();
    updateNavigation();
  }
  function filteredRequests() {
    const query = $("request-search").value.trim().toLowerCase();
    const state = $("status-filter").value;
    return chronologicalRequests.filter(request => {
      const search = `${request.id} ${request.source} ${request.bus}`.toLowerCase();
      return (!query || search.includes(query)) && (state === "all" || API.requestStateAt(request, current) === state);
    });
  }
  function renderCurrent() {
    if (!model) return;
    const state = API.stateAt(model, current);
    const counts = state.counts;
    $("current-time").textContent = time(current);
    $("current-unit").textContent = unitLabel();
    $("current-ps").textContent = `${current} ps`;
    if (document.activeElement !== $("jump-time")) $("jump-time").value = time(current);
    $("time-slider").value = String(Math.round(API.fraction(current, model.start, model.end) * 10000));
    $("time-slider").setAttribute("aria-valuetext", `${current} ピコ秒`);
    $("range-start").textContent = time(model.start, true);
    $("range-end").textContent = time(model.end, true);
    $("duration-badge").textContent = `観測期間 ${time(model.end - model.start, true)}`;
    $("count-generated").textContent = counts.generated;
    $("count-pending").textContent = counts.pending + counts.processing;
    $("processing-note").textContent = `待機 ${counts.pending} / 処理 ${counts.processing}`;
    $("count-in-flight").textContent = counts.in_flight;
    $("count-success").textContent = counts.success;
    $("count-dropped").textContent = counts.dropped;
    $("count-received").textContent = counts.received;
    $("received-note").textContent = `拒否 ${counts.filtered} / 受信待機 ${counts.rx_pending}`;
    renderNodes(state);
    renderNetwork(API.networkAt(model, current));
    renderTable();
    renderDetail();
    updateCursor();
    updateNavigation();
    if ($("status-filter").value !== "all") renderTimeline();
  }
  function count(value) { return Array.isArray(value) ? value.length : value ?? 0; }

  function renderNetwork(network) {
    const canvas = $("network");
    const width = Math.max(300, $("network-wrap").clientWidth);
    const compact = width < 650;
    const nodeWidth = compact ? Math.min(190, width - 140) : Math.min(204, (width - 80) / Math.max(1, network.nodes.length) - 24);
    const cardWidth = Math.max(122, nodeWidth);
    const diagramWidth = compact ? width : Math.max(width, network.nodes.length * 155 + 70);
    const busPadding = compact ? 42 : 70;
    const busPosition = compact ? diagramWidth - busPadding : 220;
    const height = compact ? Math.max(240, network.nodes.length * 118 + 44) : 302 + Math.max(0, network.buses.length - 1) * 55;
    canvas.setAttribute("viewBox", `0 0 ${diagramWidth} ${height}`);
    canvas.setAttribute("height", String(height));
    canvas.style.minWidth = compact ? "0" : `${diagramWidth}px`;
    const fragment = document.createDocumentFragment();
    const positions = new Map();
    const busPositions = new Map();
    network.nodes.forEach((node, index) => {
      const x = compact ? 20 : (index + .5) * (diagramWidth - 70) / Math.max(1, network.nodes.length) + 35 - cardWidth / 2;
      const y = compact ? 25 + index * 118 : 27;
      positions.set(node.id, { x, y, width: cardWidth, height: 93,
        port: compact ? { x: x + cardWidth, y: y + 47 } : { x: x + cardWidth / 2, y: y + 93 } });
    });
    network.buses.forEach((bus, index) => {
      busPositions.set(bus.id, compact ? { x: busPosition - index * 20, y: height - 17 } : { x: diagramWidth / 2, y: busPosition + index * 55 });
    });
    function connectionPoint(nodeId, busId) {
      const node = positions.get(nodeId), bus = busPositions.get(busId);
      if (!node || !bus) return null;
      return compact ? { x: bus.x, y: node.port.y } : { x: node.port.x, y: bus.y };
    }
    function pointAlong(a, b, progress) {
      const p = progress === null ? .5 : Math.max(0, Math.min(1, progress));
      return { x: a.x + (b.x - a.x) * p, y: a.y + (b.y - a.y) * p };
    }
    function selectable(group, requestId, label) {
      if (!requestId) return;
      group.setAttribute("role", "button"); group.setAttribute("tabindex", "0");
      group.setAttribute("aria-label", label);
      group.addEventListener("click", () => selectRequest(requestId));
      group.addEventListener("keydown", event => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); selectRequest(requestId); }
      });
    }
    for (const bus of network.buses) {
      const pos = busPositions.get(bus.id);
      fragment.append(svg("line", compact
        ? { x1: pos.x, x2: pos.x, y1: 25, y2: height - 24, stroke: bus.state === "transmitting" ? "#79aaa9" : "#b5c7d2", "stroke-width": 4, "stroke-linecap": "round" }
        : { x1: 39, x2: diagramWidth - 39, y1: pos.y, y2: pos.y, stroke: bus.state === "transmitting" ? "#79aaa9" : "#b5c7d2", "stroke-width": 4, "stroke-linecap": "round" }));
      const group = svg("g", { "data-bus": bus.id, "data-state": bus.state, class: "network-selectable" });
      const labelX = compact ? pos.x - 44 : pos.x - 125;
      const labelY = compact ? height - 39 : pos.y + 19;
      const labelWidth = compact ? 86 : 250;
      group.append(svg("rect", { x: labelX, y: labelY, width: labelWidth, height: compact ? 27 : 42, rx: 7, fill: "#eef4f7", stroke: "#d8e4eb", class: "network-focus" }));
      group.append(svg("text", { x: labelX + labelWidth / 2, y: labelY + 17, fill: "#49637b", "font-size": compact ? 9 : 11, "text-anchor": "middle", "font-weight": 550 }, compact ? "CAN BUS" : bus.id));
      if (!compact) group.append(svg("text", { x: pos.x, y: labelY + 32, fill: "#8294a5", "font-size": 9, "text-anchor": "middle" }, LABELS[bus.state] || bus.state));
      group.append(svg("title", {}, `${bus.id} · ${LABELS[bus.state] || bus.state}${bus.requestId ? ` · ${bus.requestId}` : ""}`));
      selectable(group, bus.requestId, `バス ${bus.id} の要求 ${bus.requestId} を表示`);
      fragment.append(group);
    }
    for (const connection of network.connections) {
      const node = positions.get(connection.node), bus = connectionPoint(connection.node, connection.bus);
      if (!node || !bus) continue;
      const line = svg("line", { x1: node.port.x, y1: node.port.y, x2: bus.x, y2: bus.y, stroke: "#cedae2", "stroke-width": 2,
        "stroke-dasharray": connection.inferred ? "5 5" : "none", "data-connection-node": connection.node, "data-connection-bus": connection.bus, "data-inferred": String(connection.inferred) });
      line.append(svg("title", {}, `${connection.node} ↔ ${connection.bus}${connection.inferred ? "（単一バスの情報から推定）" : "（要求・受信の記録から確認）"}`));
      fragment.append(line);
    }
    for (const node of network.nodes) {
      const pos = positions.get(node.id);
      const rx = network.rx.filter(packet => packet.receiver === node.id);
      const sent = network.tx.find(packet => packet.source === node.id);
      const trail = network.trails.find(item => item.receiver === node.id || item.source === node.id);
      const latest = [...chronologicalRequests].reverse().find(request => request.generated <= current && (request.source === node.id || request.receivers.some(receiver => receiver.receiver === node.id)));
      const requestId = sent?.requestId || rx[0]?.requestId || trail?.requestId || latest?.id;
      const state = sent ? "送信中" : rx.length ? "受信処理中" : node.processing ? "TX 処理中" : node.queue ? "送信待機" : "アイドル";
      const active = !!sent || rx.length > 0;
      const group = svg("g", { "data-node": node.id, "data-state": state, class: "network-selectable" });
      group.append(svg("rect", { x: pos.x, y: pos.y, width: pos.width, height: pos.height, rx: 10, fill: active ? "#f0f9f7" : "#fff", stroke: active ? "#8bc3b8" : "#dce5eb", "stroke-width": active ? 1.5 : 1, class: "network-focus" }));
      group.append(svg("rect", { x: pos.x + 12, y: pos.y + 14, width: 23, height: 21, rx: 5, fill: sent ? "#187f88" : "#eaf0f5" }));
      group.append(svg("path", { d: `M ${pos.x + 17} ${pos.y + 21} h 13 M ${pos.x + 17} ${pos.y + 27} h 13`, fill: "none", stroke: sent ? "#fff" : "#8ca0b2", "stroke-width": 1.4 }));
      const name = svg("text", { x: pos.x + 43, y: pos.y + 24, fill: "#304960", "font-size": 10, "font-weight": 600 }, shorten(node.id, pos.width - 55));
      name.append(svg("title", {}, node.id)); group.append(name);
      group.append(svg("text", { x: pos.x + 43, y: pos.y + 39, fill: active ? "#187f88" : "#8b9bac", "font-size": 8 }, state));
      group.append(svg("line", { x1: pos.x + 12, x2: pos.x + pos.width - 12, y1: pos.y + 52, y2: pos.y + 52, stroke: "#e7eef1" }));
      group.append(svg("text", { x: pos.x + 13, y: pos.y + 69, fill: "#8797a9", "font-size": 8 }, `キュー ${count(node.queue)} · TX 処理 ${count(node.processing)}`));
      group.append(svg("text", { x: pos.x + 13, y: pos.y + 82, fill: "#8797a9", "font-size": 8 }, `受信 ${node.received} · 拒否 ${node.filtered}`));
      group.append(svg("circle", { cx: pos.port.x, cy: pos.port.y, r: 3.5, fill: "#fff", stroke: "#a8bdca", "stroke-width": 1.4 }));
      selectable(group, requestId, `ノード ${node.id} の要求 ${requestId} を表示`);
      fragment.append(group);
    }
    function packet(item, kind, position, color, extra = {}) {
      const group = svg("g", { transform: `translate(${position.x} ${position.y})`, class: "network-packet", "data-packet-kind": kind,
        "data-request-id": item.requestId, "data-source": item.source, "data-bus": item.bus, "data-receiver": item.receiver || "",
        "data-phase": item.phase || "", "data-progress": item.progress === null ? "unknown" : String(item.progress), ...extra });
      group.append(svg("circle", { cx: 0, cy: 0, r: 14, fill: color, opacity: .11 }));
      group.append(svg("rect", { x: -9, y: -6.5, width: 18, height: 13, rx: 3, fill: color, stroke: selected === item.requestId ? "#203651" : "#fff", "stroke-width": 1.5 }));
      group.append(svg("path", { d: "M -4 -2 L 0 1 L 4 -2", fill: "none", stroke: "#fff", "stroke-width": 1.2 }));
      group.append(svg("title", {}, `${item.requestId} · ${kind === "tx" ? "送信の論理進捗" : item.phase === "processing" ? "RX 処理" : "受信観測待機"}${item.progress === null ? "（終了時刻未確定）" : ` ${Math.round(item.progress * 100)}%`}`));
      selectable(group, item.requestId, `パケット ${item.requestId} の詳細を表示`);
      fragment.append(group);
    }
    // All active packets come from the full model, independent of table filters and timeline caps.
    for (const tx of network.tx) {
      const node = positions.get(tx.source), bus = connectionPoint(tx.source, tx.bus);
      if (node && bus) packet(tx, "tx", pointAlong(node.port, bus, tx.progress), "#187f88");
    }
    for (const rx of network.rx) {
      const node = positions.get(rx.receiver), bus = connectionPoint(rx.receiver, rx.bus);
      if (!node || !bus) continue;
      if (rx.phase === "observation") packet(rx, "rx", pointAlong(bus, node.port, rx.progress), "#58a78e");
      else {
        const p = rx.progress === null ? .5 : rx.progress;
        const position = { x: node.x + node.width - 21, y: node.y + 24 };
        packet(rx, "rx", position, "#58a78e");
        fragment.append(svg("circle", { cx: position.x, cy: position.y, r: 12, fill: "none", stroke: "#378a76", "stroke-width": 1.5,
          "stroke-dasharray": `${p * 75.398} 75.398`, transform: `rotate(-90 ${position.x} ${position.y})`, "pointer-events": "none" }));
      }
    }
    for (const trail of network.trails) {
      const nodeId = trail.receiver || trail.source;
      const node = positions.get(nodeId), bus = connectionPoint(nodeId, trail.bus);
      if (!node || !bus) continue;
      const position = trail.kind === "sent" ? bus : node.port;
      const color = trail.kind === "filtered" ? "#cf7180" : trail.kind === "sent" ? "#187f88" : "#58a78e";
      const group = svg("g", { transform: `translate(${position.x} ${position.y})`, opacity: trail.opacity, class: "network-packet",
        "data-packet-kind": "trail", "data-trail-kind": trail.kind, "data-request-id": trail.requestId, "data-source": trail.source,
        "data-bus": trail.bus, "data-receiver": trail.receiver || "", "data-time-ps": trail.time.toString(), "data-age-ps": trail.age.toString() });
      group.append(svg("circle", { cx: 0, cy: 0, r: 11, fill: "#fff", stroke: color, "stroke-width": 2 }));
      group.append(svg("text", { x: 0, y: 3.5, fill: color, "font-size": 11, "text-anchor": "middle", "font-weight": 600 }, trail.kind === "filtered" ? "×" : "✓"));
      const label = trail.kind === "sent" ? "EOF 到達" : trail.kind === "filtered" ? "フィルタ拒否" : "受信完了";
      group.append(svg("title", {}, `${trail.requestId} · ${label} · ${trail.time} ps`));
      selectable(group, trail.requestId, `要求 ${trail.requestId} の${label}を表示`);
      fragment.append(group);
    }
    if (!network.nodes.length && !network.buses.length) fragment.append(svg("text", { x: diagramWidth / 2, y: height / 2, fill: "#8a99aa", "font-size": 11, "text-anchor": "middle" }, "描画できるノードの記録がありません"));
    canvas.replaceChildren(fragment);
    $("network-state").textContent = `TX ${network.tx.length} · RX ${network.rx.length}`;
    $("network-trail-window").textContent = `直近 ${time(network.trailWindowPs, true)} の結果`;
  }
  function renderNodes(state) {
    const cards = [];
    for (const node of state.nodes) {
      const card = element("article", "node-card");
      const name = element("div", "node-name");
      const title = element("strong", "", node.id);
      title.title = node.id;
      name.append(title, element("i", `node-dot${node.transmitting.length ? " active" : ""}`));
      const counters = element("div", "node-counts");
      for (const [label, value] of [["キュー", count(node.queue)], ["TX 処理", count(node.processing)], ["受信済み", node.received]]) {
        const item = element("div"); item.append(element("span", "", label), element("strong", "", value)); counters.append(item);
      }
      const description = node.transmitting.length ? `送信中 ${node.transmitting.join(", ")}` : `送信なし · フィルタ拒否 ${node.filtered}`;
      card.append(name, counters, element("div", "node-description", description));
      cards.push(card);
    }
    $("node-states").replaceChildren(...cards);
    $("node-count").textContent = `${state.nodes.length} ノード`;
    const buses = state.buses.map(bus => {
      const item = element("div", "bus-state");
      item.append(element("strong", "", bus.id), statusBadge(bus.state));
      if (bus.requestId) item.append(element("span", "", bus.requestId));
      return item;
    });
    $("bus-states").replaceChildren(...buses);
  }
  function renderTable() {
    const requests = filteredRequests();
    const pages = Math.max(1, Math.ceil(requests.length / PAGE_SIZE));
    if (page >= pages) page = pages - 1;
    const start = page * PAGE_SIZE;
    const rows = requests.slice(start, start + PAGE_SIZE).map(request => {
      const row = element("tr", `selectable${request.id === selected ? " selected" : ""}`);
      row.tabIndex = 0;
      row.setAttribute("aria-label", `要求 ${request.id} の詳細を表示`);
      row.setAttribute("aria-selected", String(request.id === selected));
      row.addEventListener("click", () => selectRequest(request.id));
      row.addEventListener("keydown", event => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); selectRequest(request.id); }
      });
      for (const value of [request.id, request.source, request.bus]) {
        const cell = element("td", "", value); cell.title = value; row.append(cell);
      }
      const state = element("td"); state.append(statusBadge(API.requestStateAt(request, current))); row.append(state);
      for (const value of [request.generated, request.sof, request.eof]) {
        const cell = element("td", "", time(value));
        cell.title = value === null ? "未確定" : `${value} ps`;
        row.append(cell);
      }
      row.append(element("td", "", request.receivers.length));
      return row;
    });
    if (!rows.length) {
      const row = element("tr"); const cell = element("td", "table-empty", model.requests.length ? "条件に一致する要求はありません" : "この結果には要求がありません"); cell.colSpan = 8; row.append(cell); rows.push(row);
    }
    $("request-rows").replaceChildren(...rows);
    $("request-total").textContent = `${model.requests.length} 件`;
    $("table-count").textContent = requests.length ? `${start + 1}–${Math.min(start + PAGE_SIZE, requests.length)} / ${requests.length} 件表示${requests.length !== model.requests.length ? `（全 ${model.requests.length} 件）` : ""}` : "0 件表示";
    $("page-label").textContent = `${page + 1} / ${pages}`;
    $("page-prev").disabled = page === 0;
    $("page-next").disabled = page >= pages - 1;
    document.querySelectorAll(".table-unit").forEach(label => { label.textContent = unitLabel(); });
  }
  function selectRequest(id) {
    selected = id;
    renderTable();
    renderDetail();
    renderTimeline();
    renderNetwork(API.networkAt(model, current));
    $("announcement").textContent = `要求 ${id} を選択しました。`;
  }
  function fields(target, values) {
    const nodes = [];
    for (const [label, value] of values) nodes.push(element("dt", "", label), element("dd", "", value));
    target.replaceChildren(...nodes);
  }
  function renderDetail() {
    const request = model.requests.find(request => request.id === selected);
    $("detail-empty").hidden = !!request;
    $("detail-content").hidden = !request;
    $("clear-selection").hidden = !request;
    if (!request) return;
    $("selected-id").textContent = request.id;
    const state = API.requestStateAt(request, current);
    $("selected-status").className = `status ${state}`;
    $("selected-status").textContent = LABELS[state] || state;
    fields($("detail-fields"), [["送信ノード", request.source], ["バス", request.bus], ["最終記録", LABELS[request.status] || request.status], ["受信先", `${request.receivers.length} ノード`]]);
    const times = [["生成", request.generated], ["TX 処理完了", request.ready], ["SOF", request.sof], ["EOF", request.eof], ["バス解放", request.release]].map(([label, value]) => [label, exact(value)]);
    if (request.plannedEof !== null) times.push(["予定 EOF", exact(request.plannedEof)]);
    if (request.plannedRelease !== null) times.push(["予定バス解放", exact(request.plannedRelease)]);
    fields($("detail-times"), times);
    $("planned-note").hidden = request.plannedEof === null && request.plannedRelease === null;
    const receivers = request.receivers.map(receiver => {
      const item = element("article", "receiver-row");
      const heading = element("div", "receiver-heading");
      heading.append(element("strong", "", receiver.receiver), statusBadge(API.receiverStateAt(receiver, current), true));
      const times = element("div", "receiver-times");
      times.append(element("div", "", `観測 ${exact(receiver.observed)}${receiver.observed === null ? "" : " ps"}`), element("div", "", `受信完了 ${exact(receiver.received)}${receiver.received === null ? "" : " ps"}`));
      item.append(heading, times); return item;
    });
    if (!receivers.length) receivers.push(element("p", "node-description", "受信レコードはありません"));
    $("detail-receivers").replaceChildren(...receivers);
    $("detail-raw").textContent = JSON.stringify(request.raw, null, 2);
  }

  function renderTimeline() {
    if (!model) return;
    const canvas = $("timeline");
    const width = Math.max(550, $("timeline-wrap").clientWidth);
    const left = width < 700 ? 117 : 155;
    const right = width - 22;
    const top = 37;
    const busHeight = 32;
    const nodeHeight = 60;
    const height = top + model.buses.length * busHeight + model.nodes.length * nodeHeight + 12;
    plotGeometry = { left, right, top, height };
    canvas.setAttribute("viewBox", `0 0 ${width} ${Math.max(160, height)}`);
    canvas.setAttribute("height", String(Math.max(160, height)));
    const fragment = document.createDocumentFragment();
    fragment.append(svg("rect", { x: 0, y: 0, width, height: Math.max(160, height), fill: "#fff" }));
    const x = value => left + API.fraction(value, viewStart, viewEnd) * (right - left);
    for (let i = 0; i <= 5; i++) {
      const pos = left + (right - left) * i / 5;
      const tick = API.timeFromFraction(viewStart, viewEnd, i, 5);
      fragment.append(svg("line", { x1: pos, x2: pos, y1: top - 7, y2: height, stroke: "#eaf0f4", "stroke-dasharray": i ? "3 4" : "none" }));
      fragment.append(svg("text", { x: pos, y: 19, fill: "#8a99aa", "font-size": 9, "text-anchor": i === 5 ? "end" : i === 0 ? "start" : "middle" }, time(tick)));
    }
    fragment.append(svg("text", { x: left - 18, y: 19, fill: "#8a99aa", "font-size": 9, "text-anchor": "end" }, unitLabel()));
    const busY = new Map();
    model.buses.forEach((bus, index) => {
      const y = top + index * busHeight;
      busY.set(bus, y + 7);
      fragment.append(svg("rect", { x: 0, y, width, height: busHeight, fill: "#f4f8fa" }));
      const title = svg("text", { x: 17, y: y + 19, fill: "#566c83", "font-size": 10, "font-weight": 550 }, shorten(bus, left));
      title.append(svg("title", {}, bus)); fragment.append(title);
      fragment.append(svg("line", { x1: left, x2: right, y1: y + 16, y2: y + 16, stroke: "#d5e1e8" }));
    });
    const nodeY = new Map();
    model.nodes.forEach((node, index) => {
      const y = top + model.buses.length * busHeight + index * nodeHeight;
      nodeY.set(node, { tx: y + 10, rx: y + 34 });
      fragment.append(svg("line", { x1: 0, x2: width, y1: y + nodeHeight, y2: y + nodeHeight, stroke: "#eaf0f4" }));
      const label = svg("text", { x: 17, y: y + 25, fill: "#435973", "font-size": 10, "font-weight": 550 }, shorten(node, left - 22));
      label.append(svg("title", {}, node)); fragment.append(label);
      fragment.append(svg("text", { x: left - 8, y: y + 19, fill: "#99a8b7", "font-size": 8, "text-anchor": "end" }, "TX"));
      fragment.append(svg("text", { x: left - 8, y: y + 43, fill: "#99a8b7", "font-size": 8, "text-anchor": "end" }, "RX"));
      for (const lane of [y + 17, y + 41]) fragment.append(svg("line", { x1: left, x2: right, y1: lane, y2: lane, stroke: "#f0f3f6" }));
    });
    const requests = filteredRequests();
    const visibleIds = new Set();
    let drawn = 0;
    let omitted = 0;
    function bar(id, begin, end, y, phase, title, thin = false) {
      if (begin === null || begin === undefined || y === undefined) return false;
      if (end === null || end === undefined) end = model.end;
      if (end < viewStart || begin > viewEnd) return false;
      if (drawn >= DRAW_LIMIT) { omitted++; return false; }
      drawn++;
      visibleIds.add(id);
      const start = begin < viewStart ? viewStart : begin;
      const stop = end > viewEnd ? viewEnd : end;
      const rect = svg("rect", {
        x: Math.min(right - 3, x(start)), y: y + (thin ? 5 : 0), width: Math.max(3, x(stop) - x(start)), height: thin ? 3 : 14,
        rx: 3, fill: COLORS[phase], opacity: selected && selected !== id ? .34 : .92,
        stroke: selected === id ? "#203651" : "none", "stroke-width": 1.5,
        tabindex: 0, role: "button", "aria-label": `${id} ${title}`, cursor: "pointer"
      });
      rect.append(svg("title", {}, `${id} · ${title}\n${begin}–${end} ps`));
      rect.addEventListener("click", event => { event.stopPropagation(); selectRequest(id); });
      rect.addEventListener("keydown", event => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); selectRequest(id); }
      });
      fragment.append(rect);
      if (!thin && x(stop) - x(start) > 43) {
        const capacity = Math.min(16, Math.floor((x(stop) - x(start) - 10) / 5));
        const label = svg("text", { x: x(start) + 5, y: y + 10, fill: "#fff", "font-size": 8, "pointer-events": "none", opacity: selected && selected !== id ? .34 : 1 }, id.length > capacity ? `${id.slice(0, capacity - 1)}…` : id);
        fragment.append(label);
      }
      return true;
    }
    for (const request of requests) {
      const lane = nodeY.get(request.source);
      if (!lane) continue;
      if (request.generated !== null && (request.ready === null || request.ready > request.generated)) bar(request.id, request.generated, request.ready, lane.tx, "processing", "TX 処理");
      if (request.ready !== null && request.status !== "dropped" && (request.sof === null || request.sof > request.ready)) bar(request.id, request.ready, request.sof, lane.tx, "pending", "送信待機");
      if (request.sof !== null) {
        bar(request.id, request.sof, request.eof, lane.tx, "in_flight", "フレーム送信");
        bar(request.id, request.sof, request.eof, busY.get(request.bus), "in_flight", "バス占有");
      }
      if (request.eof !== null && (request.release === null || request.release > request.eof)) bar(request.id, request.eof, request.release, busY.get(request.bus), "intermission", "バス間隔");
      if (request.status === "dropped") bar(request.id, request.ready ?? request.generated, request.ready ?? request.generated, lane.tx, "dropped", "破棄");
      for (const receiver of request.receivers) {
        const rxLane = nodeY.get(receiver.receiver);
        if (!rxLane) continue;
        if (receiver.eof !== null && (receiver.observed === null || receiver.observed > receiver.eof)) bar(request.id, receiver.eof, receiver.observed, rxLane.rx, "received", "受信観測待機", true);
        if (receiver.observed !== null) {
          if (receiver.status === "filtered") bar(request.id, receiver.observed, receiver.observed, rxLane.rx, "dropped", "フィルタ拒否");
          else bar(request.id, receiver.observed, receiver.received, rxLane.rx, "received", "RX 処理");
        }
      }
    }
    cursorLine = svg("line", { x1: left, x2: left, y1: top - 7, y2: Math.max(top, height - 2), stroke: "#203651", "stroke-width": 1.5, "stroke-dasharray": "4 3", "pointer-events": "none" });
    fragment.append(cursorLine);
    if (!model.nodes.length && !model.buses.length) fragment.append(svg("text", { x: width / 2, y: 87, fill: "#8a99aa", "font-size": 11, "text-anchor": "middle" }, "ノードの記録がありません"));
    canvas.replaceChildren(fragment);
    $("viewport-label").textContent = `表示範囲 ${time(viewStart, true)} – ${time(viewEnd, true)}`;
    $("timeline-count").textContent = `${visibleIds.size} 要求 / ${drawn} 区間${omitted ? ` · 描画上限 ${DRAW_LIMIT} 区間（${omitted} 区間省略）` : ""}`;
    updateCursor();
  }
  function shorten(text, labelWidth) {
    const maximum = Math.max(8, Math.floor(labelWidth / 7));
    return text.length > maximum ? `…${text.slice(-(maximum - 1))}` : text;
  }
  function updateCursor() {
    if (!cursorLine || !model || !plotGeometry) return;
    const { left, right } = plotGeometry;
    const outside = current < viewStart || current > viewEnd;
    cursorLine.setAttribute("visibility", outside ? "hidden" : "visible");
    if (!outside) {
      const pos = left + API.fraction(current, viewStart, viewEnd) * (right - left);
      cursorLine.setAttribute("x1", String(pos)); cursorLine.setAttribute("x2", String(pos));
    }
  }
  function jump() {
    if (!model) return;
    pause();
    try {
      const value = API.parseTime($("jump-time").value, unit());
      if (value < model.start || value > model.end) throw new Error(`観測範囲 ${model.start}–${model.end} ps 内の時刻を指定してください。`);
      $("jump-error").hidden = true;
      $("jump-time").removeAttribute("aria-invalid");
      setCurrent(value, true);
    } catch (error) {
      $("jump-error").textContent = error.message;
      $("jump-error").hidden = false;
      $("jump-time").setAttribute("aria-invalid", "true");
    }
  }

  $("open-file").addEventListener("click", () => $("file-input").click());
  $("empty-open").addEventListener("click", () => $("file-input").click());
  $("file-input").addEventListener("change", event => { const file = event.target.files[0]; event.target.value = ""; if (file) loadFile(file); });
  $("play").addEventListener("click", () => playing ? pause() : startPlayback());
  $("prev-time").addEventListener("click", () => eventStep(-1));
  $("next-time").addEventListener("click", () => eventStep(1));
  $("speed").addEventListener("change", () => { if (playing) { pause(); startPlayback(); } });
  $("time-slider").addEventListener("input", event => { if (!model) return; pause(); $("jump-error").hidden = true; setCurrent(API.timeFromFraction(model.start, model.end, Number(event.target.value)), true); });
  $("jump").addEventListener("click", jump);
  $("jump-time").addEventListener("keydown", event => { if (event.key === "Enter") jump(); });
  $("time-unit").addEventListener("change", () => { if (model) { $("jump-time").value = time(current); $("jump-error").hidden = true; renderCurrent(); renderTimeline(); } });
  $("zoom-in").addEventListener("click", () => zoom(.5));
  $("zoom-out").addEventListener("click", () => zoom(2));
  $("zoom-reset").addEventListener("click", () => { if (model) { viewStart = model.start; viewEnd = model.end; renderTimeline(); updateNavigation(); } });
  $("clear-selection").addEventListener("click", () => { selected = null; renderTable(); renderDetail(); renderTimeline(); renderNetwork(API.networkAt(model, current)); });
  $("request-search").addEventListener("input", () => { if (model) { page = 0; renderTable(); renderTimeline(); } });
  $("status-filter").addEventListener("change", () => { if (model) { page = 0; renderTable(); renderTimeline(); } });
  $("page-prev").addEventListener("click", () => { if (model && page > 0) { page--; renderTable(); } });
  $("page-next").addEventListener("click", () => { if (model) { page++; renderTable(); } });
  $("timeline").addEventListener("click", event => {
    if (!model || !plotGeometry) return;
    const bounds = $("timeline").getBoundingClientRect();
    const width = Math.max(550, $("timeline-wrap").clientWidth);
    const position = (event.clientX - bounds.left) * width / bounds.width;
    const { left, right } = plotGeometry;
    if (position < left || position > right) return;
    pause();
    setCurrent(API.timeFromFraction(viewStart, viewEnd, Math.round((position - left) / (right - left) * 10000)));
  });
  document.addEventListener("dragenter", event => {
    if (!event.dataTransfer || !Array.from(event.dataTransfer.types).includes("Files")) return;
    event.preventDefault(); dragDepth++; $("drop-overlay").hidden = false;
  });
  document.addEventListener("dragover", event => { event.preventDefault(); if (event.dataTransfer) event.dataTransfer.dropEffect = "copy"; });
  document.addEventListener("dragleave", event => { event.preventDefault(); if (--dragDepth <= 0) { dragDepth = 0; $("drop-overlay").hidden = true; } });
  document.addEventListener("drop", event => {
    event.preventDefault(); dragDepth = 0; $("drop-overlay").hidden = true;
    const files = event.dataTransfer ? Array.from(event.dataTransfer.files) : [];
    if (files.length === 1) loadFile(files[0]);
    else if (files.length > 1) { ++loadVersion; failLoad("results.json を一つずつ選択してください。"); }
  });
  document.addEventListener("keydown", event => {
    if (!model || event.defaultPrevented || ["INPUT", "SELECT", "TEXTAREA", "BUTTON"].includes(event.target.tagName) || event.target.getAttribute("role") === "button") return;
    if (event.code === "Space") { event.preventDefault(); playing ? pause() : startPlayback(); }
    if (event.key === "ArrowRight") { event.preventDefault(); eventStep(1); }
    if (event.key === "ArrowLeft") { event.preventDefault(); eventStep(-1); }
  });
  if (typeof ResizeObserver !== "undefined") {
    new ResizeObserver(() => { if (model) renderTimeline(); }).observe($("timeline-wrap"));
    new ResizeObserver(() => { if (model) renderNetwork(API.networkAt(model, current)); }).observe($("network-wrap"));
  } else window.addEventListener("resize", () => { if (model) { renderTimeline(); renderNetwork(API.networkAt(model, current)); } });
  document.addEventListener("visibilitychange", () => { if (document.hidden) pause(); });
  clearResult();
  const embedded = $("embedded-results");
  if (embedded) {
    try { loadObject(JSON.parse(embedded.textContent), "results.json（埋め込み）"); }
    catch (error) { failLoad(error instanceof Error ? error.message : String(error)); }
  }
})();
