// app.js — renderer logic (window.__TAURI__ global API, zero build step)
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getVersion } = window.__TAURI__.app;

const $ = id => document.getElementById(id);
let selectedTarget = ""; // relative to base_path root ("/" shows as misc root)

function showView(name) {
  $("view-main").classList.toggle("hidden", name !== "main");
  $("view-settings").classList.toggle("hidden", name !== "settings");
}

function setMsg(id, text, cls) {
  const el = $(id);
  el.textContent = text;
  el.className = "msg " + (cls || "");
}

// ---------- settings view ----------
async function openSettings() {
  const s = await invoke("load_settings");
  $("cfg-url").value = s.base_url || "";
  $("cfg-user").value = s.username || "";
  $("cfg-pass").value = "";
  $("cfg-pass").placeholder = s.has_password ? "留空 = 不修改" : "password";
  setMsg("cfg-msg", "");
  try { await loadLogSync(); } catch (e) { console.warn(e); }
  showView("settings");
}

async function saveSettings() {
  setMsg("cfg-msg", "连接中…");
  try {
    await invoke("save_settings", {
      baseUrl: $("cfg-url").value.trim(),
      username: $("cfg-user").value.trim(),
      password: $("cfg-pass").value,
    });
    setMsg("cfg-msg", "已连接 ✓", "ok");
    await refreshStatus();
    await renderTree();
    setTimeout(() => showView("main"), 600);
  } catch (e) {
    setMsg("cfg-msg", "失败: " + e, "err");
  }
}

async function refreshStatus() {
  const s = await invoke("load_settings");
  const el = $("status");
  el.textContent = s.connected ? "已连接" : "未连接";
  el.className = "badge " + (s.connected ? "connected" : "disconnected");
  $("conn-user").textContent = s.connected ? s.username : "";
}

// ---------- directory tree ----------
function nodeEl(name, path, depth) {
  const wrap = document.createElement("div");
  const row = document.createElement("div");
  row.className = "dir-node";
  row.style.paddingLeft = 6 + depth * 2 + "px";
  row.innerHTML = `<span class="dir-toggle">▸</span>${escapeHtml(name || "/")}`;
  const children = document.createElement("div");
  children.className = "dir-children";
  children.style.display = "none";
  let expanded = false;

  row.addEventListener("click", async () => {
    document.querySelectorAll(".dir-node.selected").forEach(n => n.classList.remove("selected"));
    row.classList.add("selected");
    selectedTarget = path;
    $("target-display").textContent = "/misc" + (path ? "/" + path : "");
    await invoke("set_target", { mode: "manual", target: path });
    document.querySelector('input[name="mode"][value="manual"]').checked = true;
    if (!expanded) {
      expanded = true;
      row.querySelector(".dir-toggle").textContent = "▾";
      try {
        const entries = await invoke("list_dir", { path });
        for (const d of entries.filter(e => e.is_dir)) children.appendChild(nodeEl(d.name, path ? path + "/" + d.name : d.name, depth + 1));
      } catch (e) { console.warn(e); }
    } else {
      children.style.display = children.style.display === "none" ? "" : "none";
    }
  });

  wrap.appendChild(row);
  wrap.appendChild(children);
  return wrap;
}

async function renderTree() {
  const tree = $("tree");
  tree.innerHTML = "";
  tree.appendChild(nodeEl("", "", 0));
  await invoke("set_target", { mode: "manual", target: "" });
  selectedTarget = "";
  $("target-display").textContent = "/misc";
}

// ---------- drag & drop visual ----------
const dz = $("dropzone");
const hasFiles = e => e.dataTransfer && Array.from(e.dataTransfer.types || []).includes("Files");
window.addEventListener("dragover", e => { e.preventDefault(); if (hasFiles(e)) dz.classList.add("drag"); });
window.addEventListener("dragleave", e => { if (e.target === document.body) dz.classList.remove("drag"); });
window.addEventListener("drop", e => { e.preventDefault(); dz.classList.remove("drag"); });

// ---------- mode radio ----------
document.querySelectorAll('input[name="mode"]').forEach(r => {
  r.addEventListener("change", () => {
    const mode = document.querySelector('input[name="mode"]:checked').value;
    invoke("set_target", { mode, target: selectedTarget });
  });
});

// ---------- queue ----------
function fmtSize(n) {
  if (n > 1 << 30) return (n / (1 << 30)).toFixed(2) + "G";
  if (n > 1 << 20) return (n / (1 << 20)).toFixed(1) + "M";
  if (n > 1024) return (n / 1024).toFixed(1) + "K";
  return n + "B";
}

function renderQueue(items) {
  const q = $("queue");
  q.innerHTML = "";
  for (const it of items) {
    const row = document.createElement("div");
    row.className = "q-row";
    const err = it.error ? `<span class="q-err" title="${escapeHtml(it.error)}">${escapeHtml(it.error)}</span>` : "";
    const pct = it.size ? Math.min(100, Math.round(100 * (it.uploaded || 0) / it.size)) : 0;
    const bar = it.state === "uploading" ? `<div class="q-bar"><div style="width:${pct}%"></div></div>` : "";
    const relTxt = it.rel && ["done", "skipped"].includes(it.state) ? " → " + escapeHtml(it.rel) : "";
    row.innerHTML = `
      <span class="q-name" title="${escapeHtml(it.name)}">${escapeHtml(it.name)}</span>
      <span class="q-size">${fmtSize(it.size)}</span>
      ${bar}
      <span class="state-pill ${it.state}">${it.state}${relTxt}</span>
      ${err}`;
    q.appendChild(row);
  }
  const counts = {};
  for (const it of items) counts[it.state] = (counts[it.state] || 0) + 1;
  const active = ["hashing", "pending", "processing", "uploading", "cooldown"].reduce((a, k) => a + (counts[k] || 0), 0);
  $("queue-stats").textContent = items.length
    ? `共 ${items.length} | 进行 ${active} | 完成 ${counts.done || 0} | 跳过 ${counts.skipped || 0} | 失败 ${counts.failed || 0}`
    : "队列空闲";
}

listen("queue-updated", e => renderQueue(e.payload));

// ---------- test connection (no save) ----------
async function testConnection() {
  setMsg("cfg-msg", "测试中…");
  try {
    const r = await invoke("test_connection", {
      baseUrl: $("cfg-url").value.trim(),
      username: $("cfg-user").value.trim(),
      password: $("cfg-pass").value,
    });
    setMsg("cfg-msg", `连接成功 ✓(根目录 ${r.rootDirs} 个子目录,上传接口 ${r.davUrl})`, "ok");
  } catch (e) {
    setMsg("cfg-msg", "连接失败: " + e, "err");
  }
}
$("btn-test").addEventListener("click", testConnection);

// ---------- hot update ----------
async function checkUpdate() {
  setMsg("update-msg", "检查中…");
  try {
    const r = await invoke("check_update");
    if (r.available) {
      setMsg("update-msg", `发现新版本 ${r.version},准备下载…`);
      await invoke("install_update"); // downloads, installs, restarts
    } else {
      setMsg("update-msg", "已是最新版本", "ok");
    }
  } catch (e) {
    setMsg("update-msg", "检查失败: " + e, "err");
  }
}

listen("update-progress", e => {
  const { downloaded, total } = e.payload;
  $("update-bar-wrap").classList.remove("hidden");
  if (total) {
    const pct = Math.min(100, Math.floor(100 * downloaded / total));
    $("update-bar").style.width = pct + "%";
    setMsg("update-msg", `下载中 ${pct}%(${(downloaded / 1048576).toFixed(2)} / ${(total / 1048576).toFixed(2)} MB)`);
  } else {
    $("update-msg").textContent = `下载中 ${(downloaded / 1048576).toFixed(2)} MB`;
  }
});

// ---------- remote log sync settings ----------
async function loadLogSync() {
  const ls = await invoke("get_log_sync");
  $("ls-enabled").checked = ls.enabled;
  $("ls-url").value = ls.base_url || "";
  $("cfg-ls-user").value = ls.username || "";
  $("cfg-ls-pass").value = "";
  $("cfg-ls-pass").placeholder = ls.password ? "留空 = 不修改" : "password";
  setMsg("ls-msg", ls.enabled ? "已启用" : "已关闭", ls.enabled ? "ok" : "");
}

async function saveLogSync() {
  setMsg("ls-msg", "保存中…");
  try {
    await invoke("set_log_sync", {
      enabled: $("ls-enabled").checked,
      baseUrl: $("ls-url").value.trim(),
      username: $("cfg-ls-user").value.trim(),
      password: $("cfg-ls-pass").value,
    });
    setMsg("ls-msg", "已保存 ✓", "ok");
  } catch (e) {
    setMsg("ls-msg", "失败: " + e, "err");
  }
}
$("ls-save").addEventListener("click", saveLogSync);

// ---------- buttons ----------
$("btn-settings").addEventListener("click", openSettings);
$("btn-back").addEventListener("click", () => showView("main"));
$("cfg-save").addEventListener("click", saveSettings);
$("btn-update").addEventListener("click", checkUpdate);
$("btn-retry").addEventListener("click", () => invoke("retry_failed"));
$("btn-clear").addEventListener("click", () => invoke("clear_finished"));
$("btn-logdir").addEventListener("click", () => invoke("open_log_dir"));

(async () => {
  try {
    const li = await invoke("log_remote_info");
    const el = $("log-target");
    if (li.enabled) {
      el.innerHTML = `📡 <code class="chip">${escapeHtml(li.base_url)}</code> 账号 <code class="chip">${escapeHtml(li.user)}</code> → <code class="chip">${escapeHtml(li.remote_dir)}/</code>`;
    } else {
      el.textContent = "远程日志未启用(本地开发构建),日志仅存在本机。";
    }
  } catch (e) { console.warn(e); }
})();

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// ---------- boot ----------
(async () => {
  const v = await getVersion().catch(() => "?");
  $("app-version").textContent = "v" + v;
  $("ver-setting").textContent = "v" + v;
  try {
    const s = await invoke("load_settings");
    if (s.base_url && s.hasPassword) await invoke("connect");
  } catch (e) {
    console.warn("auto-connect:", e);
  }
  await refreshStatus();
  if ($("status").classList.contains("connected")) await renderTree();
  else showView("settings");
  renderQueue(await invoke("get_queue"));
})();
