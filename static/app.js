"use strict";

let state = null;
let expandedModels = new Set();
let editingId = null;

const $ = (sel) => document.querySelector(sel);

// ---------- polling ----------
async function loadState() {
  try {
    const res = await fetch("/api/state");
    if (!res.ok) throw new Error("state " + res.status);
    state = await res.json();
    render();
  } catch (e) {
    // backend not reachable yet
  }
}

// ---------- rendering ----------
function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

function fmtMs(ms) {
  if (ms == null) return "—";
  return ms + " мс";
}

function render() {
  renderGlobal();
  renderRouting();
  renderProviders();
  renderEvents();
  renderBenchStatus();
}

function renderGlobal() {
  const providers = state.providers;
  const ok = providers.filter((p) => p.status === "ok").length;
  const down = providers.filter((p) => p.status === "down").length;
  const checking = providers.filter((p) => p.status === "checking").length;
  const dot = $("#global-dot");
  const text = $("#global-text");
  if (providers.length === 0) {
    dot.className = "dot gray";
    text.textContent = "нет API";
  } else if (down > 0 && ok === 0) {
    dot.className = "dot red";
    text.textContent = `${ok} ok · ${down} down · ${checking} проверка`;
  } else if (down > 0 || checking > 0) {
    dot.className = "dot yellow";
    text.textContent = `${ok} ok · ${down} down · ${checking} проверка`;
  } else {
    dot.className = "dot green";
    text.textContent = `${ok} ok`;
  }
}

function renderRouting() {
  const fast = state.routing_fast;
  const think = state.routing_think;
  fillSelect($("#fast-select"), state.fast_override, state.candidates);
  fillSelect($("#think-select"), state.think_override, state.candidates);
  $("#fast-body").innerHTML = fast
    ? routeHtml(fast, "fast")
    : "нет данных — запустите проверку";
  $("#think-body").innerHTML = think
    ? routeHtml(think, "think")
    : "нет данных — добавьте API";
}

function fillSelect(sel, override, candidates) {
  const vals = new Set(candidates || []);
  if (override) vals.add(override);
  const sorted = [...vals].sort();
  const options = ['<option value="">auto</option>']
    .concat(sorted.map((m) => `<option value="${esc(m)}">${esc(m)}</option>`))
    .join("");
  if (sel.innerHTML !== options) sel.innerHTML = options;
  const want = override || "";
  if (sel.value !== want) sel.value = want;
}

function routeHtml(t, kind) {
  return `
    <div class="route-target">
      <div class="model">${esc(t.model)}</div>
      <div class="provider">${esc(t.provider_name)}</div>
      <div class="reason">${esc(t.reason)}</div>
    </div>`;
}

function renderProviders() {
  const wrap = $("#providers");
  if (!state.providers.length) {
    wrap.innerHTML = '<div class="events"><li>Пока нет API — добавьте в разделе «API».</li></div>';
    return;
  }
  wrap.innerHTML = state.providers.map(providerHtml).join("");
}

function providerHtml(p) {
  const statusCls = p.status === "ok" ? "green" : p.status === "down" ? "red" : "yellow";
  const statusLabel = p.status === "ok" ? "ok" : p.status === "down" ? "down" : "проверка";
  const isFast = state.routing_fast && state.routing_fast.provider_id === p.id;
  const isThink = state.routing_think && state.routing_think.provider_id === p.id;
  const latency = p.benchmark_latency_ms != null ? fmtMs(p.benchmark_latency_ms) : "—";

  let models = "";
  if (p.models.length) {
    const open = expandedModels.has(p.id) ? " open" : "";
    const rows = p.models.map((m) => {
      const chosen =
        (isFast && state.routing_fast.model === m) || (isThink && state.routing_think.model === m);
      return `<div class="model-row${chosen ? " chosen" : ""}">${chosen ? "<b>" : ""}${esc(m)}${chosen ? "</b>" : ""}</div>`;
    }).join("");
    models = `
      <div class="models-toggle" data-id="${esc(p.id)}">▸ модели (${p.models.length})</div>
      <div class="models-list${open}" data-id="${esc(p.id)}">${rows}</div>`;
  }

  return `
    <div class="provider" data-id="${esc(p.id)}">
      <div class="provider-head">
        <span class="dot ${statusCls}"></span>
        <span class="name">${esc(p.name)}</span>
        <span class="key">${esc(p.api_key_masked)}</span>
        <span class="base">${esc(p.base_url)}</span>
        ${isFast ? '<span class="badge fast">fast</span>' : ""}
        ${isThink ? '<span class="badge think">think</span>' : ""}
        <div class="provider-actions">
          <button class="btn small" data-act="check" data-id="${esc(p.id)}">Проверить</button>
          <button class="btn small" data-act="edit" data-id="${esc(p.id)}">Редактировать</button>
          <button class="btn small danger" data-act="del" data-id="${esc(p.id)}">Удалить</button>
        </div>
      </div>
      <div class="provider-sub">
        <span>статус: ${statusLabel}</span>
        <span>латентность: ${latency}</span>
        ${p.model_override ? `<span>override: ${esc(p.model_override)}</span>` : ""}
        ${p.last_error ? `<span class="err">${esc(p.last_error)}</span>` : ""}
      </div>
      ${models}
    </div>`;
}

function renderEvents() {
  const ul = $("#events");
  const evs = state.events || [];
  if (!evs.length) {
    ul.innerHTML = '<li class="empty">событий пока нет</li>';
    return;
  }
  ul.innerHTML = evs.map((e) => {
    const d = new Date(e.ts * 1000);
    const time = d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
    return `<li><span class="time">${time}</span><span class="kind">${esc(e.kind)}</span><span>${esc(e.message)}</span></li>`;
  }).join("");
}

function renderBenchStatus() {
  $("#bench-status").textContent = state.benchmark_running ? "идёт замер…" : "";
}

// ---------- actions ----------
async function addProvider(e) {
  e.preventDefault();
  const name = $("#in-name").value.trim();
  const base_url = $("#in-base").value.trim();
  const api_key = $("#in-key").value.trim();
  const model_override = $("#in-model").value.trim();
  if (!name || !base_url) {
    $("#add-error").textContent = "Заполните название и Base URL";
    return;
  }
  try {
    const res = await fetch("/api/providers", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name, base_url, api_key, model_override: model_override || null }),
    });
    if (!res.ok) {
      const b = await res.json().catch(() => ({}));
      $("#add-error").textContent = b.error || "ошибка добавления";
      return;
    }
    $("#in-name").value = "";
    $("#in-key").value = "";
    $("#in-model").value = "";
    $("#add-error").textContent = "";
    await loadState();
  } catch (err) {
    $("#add-error").textContent = "сеть: " + err.message;
  }
}

async function post(url) {
  try { await fetch(url, { method: "POST" }); } catch (_) {}
}

async function del(id) {
  if (!confirm("Удалить этот API?")) return;
  try {
    await fetch("/api/providers/" + encodeURIComponent(id), { method: "DELETE" });
    expandedModels.delete(id);
    await loadState();
  } catch (_) {}
}

// ---------- event delegation ----------
document.addEventListener("click", async (e) => {
  const t = e.target;

  const tabBtn = t.closest(".tab-btn");
  if (tabBtn) {
    document.querySelectorAll(".tab-btn").forEach((b) => b.classList.remove("active"));
    document.querySelectorAll(".tab").forEach((s) => s.classList.remove("active"));
    tabBtn.classList.add("active");
    document.getElementById("tab-" + tabBtn.dataset.tab).classList.add("active");
    return;
  }

  const toggle = t.closest(".models-toggle");
  if (toggle) {
    const id = toggle.dataset.id;
    const list = document.querySelector(`.models-list[data-id="${CSS.escape(id)}"]`);
    if (list) {
      if (list.classList.contains("open")) {
        list.classList.remove("open");
        expandedModels.delete(id);
      } else {
        list.classList.add("open");
        expandedModels.add(id);
      }
    }
    return;
  }

  const btn = t.closest("button[data-act]");
  if (btn) {
    const { act, id } = btn.dataset;
    if (act === "check") await post("/api/providers/" + encodeURIComponent(id) + "/check");
    if (act === "edit") { openEdit(id); return; }
    if (act === "del") await del(id);
    await loadState();
    return;
  }
});

document.getElementById("btn-check-all").addEventListener("click", async () => {
  await post("/api/check_all");
  setTimeout(loadState, 400);
});

document.getElementById("btn-benchmark").addEventListener("click", async () => {
  await post("/api/benchmark");
  setTimeout(loadState, 400);
});

document.getElementById("add-form").addEventListener("submit", addProvider);

// ---------- edit provider ----------
function openEdit(id) {
  const p = (state.providers || []).find((x) => x.id === id);
  if (!p) return;
  editingId = id;
  $("#edit-name").value = p.name;
  $("#edit-base").value = p.base_url;
  $("#edit-key").value = "";
  $("#edit-model").value = p.model_override || "";
  $("#edit-error").textContent = "";
  $("#edit-modal").classList.remove("hidden");
}

function closeEdit() {
  editingId = null;
  $("#edit-modal").classList.add("hidden");
}

async function saveEdit(e) {
  e.preventDefault();
  const name = $("#edit-name").value.trim();
  const base_url = $("#edit-base").value.trim();
  const api_key = $("#edit-key").value.trim();
  const model_override = $("#edit-model").value.trim();
  if (!name || !base_url) {
    $("#edit-error").textContent = "Заполните название и Base URL";
    return;
  }
  try {
    const res = await fetch("/api/providers/" + encodeURIComponent(editingId), {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name, base_url, api_key, model_override: model_override || null }),
    });
    if (!res.ok) {
      const b = await res.json().catch(() => ({}));
      $("#edit-error").textContent = b.error || "ошибка сохранения";
      return;
    }
    closeEdit();
    await loadState();
  } catch (err) {
    $("#edit-error").textContent = "сеть: " + err.message;
  }
}

document.getElementById("edit-form").addEventListener("submit", saveEdit);
document.getElementById("edit-cancel").addEventListener("click", closeEdit);
$("#edit-modal").addEventListener("click", (e) => {
  if (e.target === $("#edit-modal")) closeEdit();
});

// ---------- routing override ----------
async function saveRouting() {
  const fast = $("#fast-select").value || null;
  const think = $("#think-select").value || null;
  try {
    await fetch("/api/routing", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ fast, think }),
    });
  } catch (_) {}
  setTimeout(loadState, 300);
}
document.getElementById("fast-select").addEventListener("change", saveRouting);
document.getElementById("think-select").addEventListener("change", saveRouting);

// ---------- boot ----------
loadState();
setInterval(loadState, 2000);