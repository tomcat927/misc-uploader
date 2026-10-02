// app.js — renderer logic (window.__TAURI__ global API, zero build step)
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = id => document.getElementById(id);
let selectedTarget = ""; // relative to base_path root ("/" shows as misc root)

// ---------- settings ----------
async function openSettings() {
  const s = await invoke("load_settings");
  $("cfg-url").value = s.base_url || "";
  $("cfg-user").value = s.username || "";
  $("cfg-pass").value = "";
  $("cfg-pass").placeholder = s.has_password ? "留空 = 不修改" : "password";
  $("settings-panel").classList.remove("hidden");
}

async function saveSettings() {
  $("cfg-msg").textContent = "连接中…";
  try {
    await invoke("save_settings", {
      baseUrl: $("cfg-url").value.trim(),
      username: $("cfg-user").value.trim(),
      password: $("cfg-pass").value,
    });
    $("cfg-msg").textContent = "已连接 ✓";
    await refreshStatus();
    await renderTree();
  } catch (e) {
    $("cfg-msg").textContent = "失败: " + e;
  }
}

async function refreshStatus() {
  const s = await invoke("load_settings");
  const el = $("status");
  el.textContent = s.connected ? "● 已连接" : "● 未连接";
  el.className = "status " + (s.connected ? "connected" : "disconnected");
  $("conn-user").textContent = s.connected ? s.username : "";
  if (s.connected) $("settings-panel").classList.add("hidden");
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
    // select this dir
    document.querySelectorAll(".dir-node.selected").forEach(n => n.classList.remove("selected"));
    row.classList.add("selected");
    selectedTarget = path;
    $("target-display").textContent = "/misc" + (path ? "/" + path : "");
    await invoke("set_target", { mode: "manual", target: path });
    document.querySelector('input[name="mode"][value="manual"]').checked = true;
    // expand
    if (!expanded) {
      expanded = true;
      row.querySelector(".dir-toggle").textContent = "▾";
      try {
        const dirs = await invoke("list_dir", { path });
        for (const d of dirs) children.appendChild(nodeEl(d, path ? path + "/" + d : d, depth + 1));
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

// ---------- drag & drop ----------
const dz = $("dropzone");
window.addEventListener("dragover", e => { e.preventDefault(); dz.classList.add("drag"); });
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
    const err = it.error ? `<span class="q-err">${escapeHtml(it.error)}</span>` : "";
    const bar = it.state === "uploading"
      ? `<div class="q-bar"><div style="width:${it.size ? Math.min(100, 100 * (it.uploaded || 0) / it.size) : 0}%"></div></div>`
      : "";
    row.innerHTML = `
      <span class="q-name">${escapeHtml(it.name)}</span>
      <span class="q-size">${fmtSize(it.size)}</span>
      ${bar}
      <span class="q-state ${it.state}">${it.state}${it.rel && (it.state === "done" || it.state === "skipped") ? " → " + escapeHtml(it.rel) : ""}</span>
      ${err}`;
    q.appendChild(row);
  }
  const counts = {};
  for (const it of items) counts[it.state] = (counts[it.state] || 0) + 1;
  const active = (counts.hashing || 0) + (counts.pending || 0) + (counts.processing || 0) + (counts.uploading || 0) + (counts.cooldown || 0);
  $("queue-stats").textContent = items.length
    ? `共 ${items.length} | 进行 ${active} | 完成 ${counts.done || 0} | 跳过 ${counts.skipped || 0} | 失败 ${counts.failed || 0}`
    : "队列空闲";
}

listen("queue-updated", e => renderQueue(e.payload));

// ---------- buttons ----------
$("btn-settings").addEventListener("click", openSettings);
$("cfg-save").addEventListener("click", saveSettings);
$("btn-retry").addEventListener("click", () => invoke("retry_failed"));
$("btn-clear").addEventListener("click", () => invoke("clear_finished"));

// ---------- hot update ----------
async function checkUpdate() {
  $("update-msg").textContent = "检查中…";
  try {
    const r = await invoke("check_update");
    if (r.available) {
      $("update-msg").textContent = `发现新版本 ${r.version},下载中…`;
      await invoke("install_update"); // downloads, installs, restarts
    } else {
      $("update-msg").textContent = "已是最新版本";
    }
  } catch (e) {
    $("update-msg").textContent = "检查失败: " + e;
  }
}
$("btn-update").addEventListener("click", checkUpdate);

listen("update-progress", e => {
  const { downloaded, total } = e.payload;
  if (total) {
    $("update-msg").textContent = `下载中 ${(downloaded / 1048576).toFixed(1)} / ${(total / 1048576).toFixed(1)} MB`;
  } else {
    $("update-msg").textContent = `下载中 ${(downloaded / 1048576).toFixed(1)} MB`;
  }
});

const { getVersion } = window.__TAURI__.app;
getVersion().then(v => { $("app-version").textContent = "v" + v; }).catch(() => {});

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// ---------- boot ----------
(async () => {
  try {
    const s = await invoke("load_settings");
    if (s.base_url && s.hasPassword) {
      await invoke("connect");
    }
  } catch (e) {
    console.warn("auto-connect:", e);
  }
  await refreshStatus();
  if ($("status").classList.contains("connected")) await renderTree();
  else openSettings();
  renderQueue(await invoke("get_queue"));
})();
