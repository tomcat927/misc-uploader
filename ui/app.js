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
  $("cfg-pass").value = s.has_password ? "********" : "";
  $("cfg-pass").placeholder = s.has_password ? "留空 = 不修改" : "password";
  setMsg("cfg-msg", "");
  try {
    const [up, gen] = await Promise.all([invoke("get_upload_prefs"), invoke("get_general_prefs")]);
    $("up-concurrency").value = up.concurrency;
    $("up-retries").value = up.max_retries;
    $("gen-autostart").checked = gen.autostart;
    $("gen-silent").checked = gen.silent_start;
    $("gen-minimize").checked = gen.minimize_on_close;
    $("gen-checkupdate").checked = gen.check_update_on_start;
  } catch (e) { console.warn(e); }
  try { await loadLogSync(); } catch (e) { console.warn(e); }
  showView("settings");
}

async function connectNow() {
  setMsg("cfg-msg", "连接中…");
  try {
    await invoke("save_settings", {
      baseUrl: $("cfg-url").value.trim(),
      username: $("cfg-user").value.trim(),
      password: $("cfg-pass").value,
    });
    await invoke("connect");
    setMsg("cfg-msg", "已连接 ✓", "ok");
    await refreshStatus();
    await renderTree();
    setTimeout(() => showView("main"), 600);
  } catch (e) {
    setMsg("cfg-msg", "连接失败: " + e, "err");
  }
}

// ---------- 自动保存:字段失焦即落盘,无保存按钮 ----------
async function autoSaveMain() {
  try {
    await invoke("save_settings", {
      baseUrl: $("cfg-url").value.trim(),
      username: $("cfg-user").value.trim(),
      password: $("cfg-pass").value,
    });
    setMsg("cfg-msg", "已自动保存 ✓", "ok");
  } catch (e) {
    setMsg("cfg-msg", e, "err");
  }
}
["cfg-url", "cfg-user", "cfg-pass"].forEach(id => $(id).addEventListener("change", autoSaveMain));

async function refreshStatus() {
  const s = await invoke("load_settings");
  const el = $("status");
  el.textContent = s.connected ? "已连接" : "未连接";
  el.className = "badge " + (s.connected ? "connected" : "disconnected");
  $("conn-user").textContent = s.connected ? s.username : "";
}

// ---------- directory tree ----------
function fileEl(f, depth, remotePath) {
  const row = document.createElement("div");
  row.className = "dir-file";
  row.style.paddingLeft = 6 + depth * 2 + "px";
  row.title = `${f.name} · ${fmtSize(f.size)}`;
  row.innerHTML = `<span class="dir-toggle"></span>${escapeHtml(f.name)} <span class="dim">(${fmtSize(f.size)})</span>`;
  row.addEventListener("click", () => {
    document.querySelectorAll(".dir-file.selected-file").forEach(n => n.classList.remove("selected-file"));
    row.classList.add("selected-file");
    showFileActions(remotePath, f.name);
  });
  return row;
}

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
  let loaded = false;
  const kidDirs = []; // { name, el } 供刷新后重定位选中目标

  async function loadChildren() {
    children.innerHTML = "";
    kidDirs.length = 0;
    try {
      const entries = await invoke("list_dir", { path });
      for (const d of entries.filter(e => e.is_dir)) {
        const el = nodeEl(d.name, path ? path + "/" + d.name : d.name, depth + 1);
        kidDirs.push({ name: d.name, el });
        children.appendChild(el);
      }
      for (const f of entries.filter(e => !e.is_dir)) children.appendChild(fileEl(f, depth + 1, path ? path + "/" + f.name : f.name));
    } catch (e) { console.warn(e); }
  }

  async function expand() {
    if (!loaded) { loaded = true; await loadChildren(); }
    expanded = true;
    children.style.display = "";
    row.querySelector(".dir-toggle").textContent = "▾";
  }
  function collapse() {
    expanded = false;
    children.style.display = "none";
    row.querySelector(".dir-toggle").textContent = "▸";
  }

  row.addEventListener("click", async () => {
    document.querySelectorAll(".dir-node.selected").forEach(n => n.classList.remove("selected"));
    row.classList.add("selected");
    selectedTarget = path;
    hideFileActions(); // 切到目录节点:取消文件选中
    $("target-display").textContent = "/misc" + (path ? "/" + path : "");
    await invoke("set_target", { mode: "manual", target: path });
    document.querySelector('input[name="mode"][value="manual"]').checked = true;
    if (!expanded) await expand();
    else collapse();
  });

  wrap.appendChild(row);
  wrap.appendChild(children);
  wrap.row = row;
  wrap.path = path;
  wrap.expand = expand;
  wrap.ensureLoaded = async () => { if (!loaded) { loaded = true; await loadChildren(); } };
  wrap.childDirs = kidDirs;
  return wrap;
}

async function renderTree(reselect) {
  const keep = reselect !== undefined ? reselect : "";
  const tree = $("tree");
  tree.innerHTML = "";
  const root = nodeEl("", "", 0);
  tree.appendChild(root);
  await invoke("set_target", { mode: "manual", target: keep });
  selectedTarget = keep;
  $("target-display").textContent = "/misc" + (keep ? "/" + keep : "");
  if (!keep) return;
  // 刷新/新建后保留选中目标:沿路径逐级展开并高亮(list_dir 按需逐级拉取)
  let node = root;
  let acc = "";
  for (const seg of keep.split("/").filter(Boolean)) {
    await node.ensureLoaded();
    acc = acc ? acc + "/" + seg : seg;
    const next = (node.childDirs || []).find(c => c.name === seg);
    if (!next) break;
    await next.el.expand();
    document.querySelectorAll(".dir-node.selected").forEach(n => n.classList.remove("selected"));
    next.el.row.classList.add("selected");
    node = next.el;
  }
}

$("btn-refresh-tree").addEventListener("click", () => renderTree(selectedTarget));

// ---------- 云端文件浏览:文件动作条 + 下载 ----------
let selectedFilePath = "";

function showFileActions(remotePath, name) {
  selectedFilePath = remotePath;
  $("fa-name").textContent = name;
  $("fa-name").title = remotePath;
  $("file-actions").classList.remove("hidden");
}
function hideFileActions() {
  selectedFilePath = "";
  $("file-actions").classList.add("hidden");
}
$("fa-close").addEventListener("click", () => {
  document.querySelectorAll(".dir-file.selected-file").forEach(n => n.classList.remove("selected-file"));
  hideFileActions();
});

$("fa-download").addEventListener("click", async () => {
  if (!selectedFilePath) return;
  try {
    const r = await invoke("download_file", { remotePath: selectedFilePath });
    addDownloadRow(r.id, r.name);
  } catch (e) {
    const row = addDownloadRow("err" + Date.now(), selectedFilePath.split("/").pop());
    row.classList.add("failed");
    row.querySelector(".dl-pct").textContent = "失败";
    row.querySelector(".dl-bar").style.display = "none";
    row.title = String(e);
  }
});

// 浏览器打开 OpenList WebUI:路由 = 路径本身(alist 系),逐段 encodeURIComponent 保中文目录
$("fa-open").addEventListener("click", async () => {
  if (!selectedFilePath) return;
  try {
    const s = await invoke("load_settings");
    const base = (s.base_url || "").replace(/\/+$/, "");
    const url = base + "/" + selectedFilePath.split("/").filter(Boolean).map(encodeURIComponent).join("/");
    await invoke("open_browser", { url });
  } catch (e) { console.warn(e); }
});

function addDownloadRow(id, name) {
  const row = document.createElement("div");
  row.className = "dl-row";
  row.id = "dl-" + id;
  row.innerHTML = `
    <div class="dl-top">
      <span class="dl-name" title="${escapeHtml(name)}">${escapeHtml(name)}</span>
      <span class="dl-pct">…</span>
      <button class="btn ghost dl-remove" title="移除">×</button>
    </div>
    <div class="dl-bar"><div></div></div>`;
  row.querySelector(".dl-remove").addEventListener("click", () => row.remove());
  $("downloads").appendChild(row);
  return row;
}

listen("download-progress", e => {
  const p = e.payload;
  const row = document.getElementById("dl-" + p.id);
  if (!row) return;
  const pct = p.total ? Math.min(100, Math.round(100 * p.downloaded / p.total)) : 0;
  const fill = row.querySelector(".dl-bar > div");
  const pctEl = row.querySelector(".dl-pct");
  if (p.state === "done") {
    row.classList.add("done");
    fill.style.width = "100%";
    pctEl.textContent = "✓ 已保存";
    row.title = p.savedTo || "";
  } else if (p.state === "failed") {
    row.classList.add("failed");
    pctEl.textContent = "失败";
    row.querySelector(".dl-bar").style.display = "none";
  } else {
    fill.style.width = pct + "%";
    pctEl.textContent = (p.total ? pct + "%" : fmtSize(p.downloaded));
  }
});

// ---------- drag & drop visual ----------
const dz = $("dropzone");
const hasFiles = e => e.dataTransfer && Array.from(e.dataTransfer.types || []).includes("Files");
window.addEventListener("dragover", e => { e.preventDefault(); if (hasFiles(e)) dz.classList.add("drag"); });
window.addEventListener("dragleave", e => { if (e.target === document.body) dz.classList.remove("drag"); });
window.addEventListener("drop", e => { e.preventDefault(); dz.classList.remove("drag"); });

// ---------- 新建文件夹(在选中的目标目录下) ----------
$("btn-newdir").addEventListener("click", () => {
  const row = $("newdir-row");
  row.classList.toggle("hidden");
  if (!row.classList.contains("hidden")) {
    $("newdir-name").placeholder = "文件夹名称,Enter 确认 / Esc 取消";
    $("newdir-name").focus();
  }
});
$("newdir-name").addEventListener("keydown", async e => {
  if (e.key === "Escape") {
    $("newdir-row").classList.add("hidden");
    return;
  }
  if (e.key !== "Enter") return;
  const name = $("newdir-name").value.trim();
  if (!name) return;
  try {
    await invoke("new_dir", { parent: selectedTarget, name });
    $("newdir-row").classList.add("hidden");
    $("newdir-name").value = "";
    await renderTree(selectedTarget); // 保留选中目标并重新拉取,新建的文件夹立即可见
  } catch (err) {
    $("newdir-name").value = "";
    $("newdir-name").placeholder = "失败: " + err;
  }
});

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

// 分组排序:进行中(hashing/pending/processing/uploading/cooldown)按入队序置顶稳定;
// 已完成(done/skipped/failed)按完成时间排,默认最新在前,可切换。切换只影响已完成区块。
const QUEUE_ACTIVE = ["hashing", "pending", "processing", "uploading", "cooldown"];
let queueAsc = localStorage.getItem("misc-uploader.queue-asc") === "1";
let lastQueueItems = [];

function groupHead(text) {
  const h = document.createElement("div");
  h.className = "q-group";
  h.textContent = text;
  return h;
}

function renderQueue(items) {
  lastQueueItems = items;
  const q = $("queue");
  q.innerHTML = "";
  const active = items.filter(it => QUEUE_ACTIVE.includes(it.state));
  const finished = items
    .filter(it => !QUEUE_ACTIVE.includes(it.state))
    .sort((a, b) =>
      queueAsc ? (a.finished_at || 0) - (b.finished_at || 0) : (b.finished_at || 0) - (a.finished_at || 0)
    );

  const addRows = list => {
    for (const it of list) {
      const row = document.createElement("div");
      row.className = "q-row";
      const err = it.error ? `<span class="q-err" title="${escapeHtml(it.error)}">${escapeHtml(it.error)}</span>` : "";
      const pct = it.size ? Math.min(100, Math.round(100 * (it.uploaded || 0) / it.size)) : 0;
      const bar = it.state === "uploading" ? `<div class="q-bar"><div style="width:${pct}%"></div></div>` : "";
      // rel 一旦确定(process 起步时)就显示去向,不必等到 done
      const dest = it.rel ? ` <span class="q-arrow">→</span> /misc/${escapeHtml(it.rel)}` : "";
      const pathTitle = escapeHtml(it.file_path || "") + (it.rel ? ` → /misc/${escapeHtml(it.rel)}` : "");
      row.innerHTML = `
        <div class="q-top">
          <span class="q-name" title="${escapeHtml(it.name)}">${escapeHtml(it.name)}</span>
          <span class="q-size">${fmtSize(it.size)}</span>
          ${bar}
          <span class="state-pill ${it.state}">${it.state}</span>
          ${err}
        </div>
        <div class="q-path" title="${pathTitle}">${escapeHtml(it.file_path || "")}${dest}</div>`;
      q.appendChild(row);
    }
  };

  if (active.length) {
    q.appendChild(groupHead(`进行中 ${active.length} · 按入队顺序`));
    addRows(active);
  }
  if (finished.length) {
    q.appendChild(groupHead(`已完成 ${finished.length} · ${queueAsc ? "旧在前 ↑" : "新在前 ↓"}`));
    addRows(finished);
  }

  const counts = {};
  for (const it of items) counts[it.state] = (counts[it.state] || 0) + 1;
  const act = QUEUE_ACTIVE.reduce((a, k) => a + (counts[k] || 0), 0);
  $("queue-stats").textContent = items.length
    ? `共 ${items.length} | 进行 ${act} | 完成 ${counts.done || 0} | 跳过 ${counts.skipped || 0} | 失败 ${counts.failed || 0}`
    : "队列空闲";
}

$("btn-qorder").textContent = queueAsc ? "↑ 旧在前" : "↓ 新在前";
$("btn-qorder").addEventListener("click", () => {
  queueAsc = !queueAsc;
  localStorage.setItem("misc-uploader.queue-asc", queueAsc ? "1" : "0");
  $("btn-qorder").textContent = queueAsc ? "↑ 旧在前" : "↓ 新在前";
  renderQueue(lastQueueItems);
});

listen("queue-updated", e => renderQueue(e.payload));

// ---------- 上传历史(跨会话,读 history.json) ----------
const HISTORY_PAGE = 200;
let historyOffset = 0;
let historyTotal = 0;
let historyLoading = false;

function fmtTime(ms) {
  const d = new Date(ms);
  const p = n => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function historyRowEl(it) {
  const row = document.createElement("div");
  row.className = "h-row";
  const name = it.rel.split("/").pop();
  const dir = it.rel.slice(0, it.rel.length - name.length);
  row.title = it.rel;
  row.innerHTML = `
    <span class="h-name">${escapeHtml(name)}</span>
    <span class="h-dir">${escapeHtml(dir)}</span>
    <span class="h-time">${escapeHtml(fmtTime(it.time))}</span>`;
  return row;
}

async function loadHistory(reset) {
  if (historyLoading) return;
  historyLoading = true;
  try {
    if (reset) {
      historyOffset = 0;
      $("history-list").innerHTML = "";
    }
    const r = await invoke("get_history", { offset: historyOffset, limit: HISTORY_PAGE });
    historyTotal = r.total;
    for (const it of r.items) $("history-list").appendChild(historyRowEl(it));
    historyOffset += r.items.length;
    $("history-count").textContent = `共 ${historyTotal} 条 · 按内容去重,同一内容记最新位置`;
    $("btn-history-more").classList.toggle("hidden", historyOffset >= historyTotal);
  } catch (e) {
    $("history-count").textContent = "加载失败: " + e;
  } finally {
    historyLoading = false;
  }
}

$("btn-history").addEventListener("click", async () => {
  const pane = $("history-pane");
  const opening = pane.classList.contains("hidden");
  pane.classList.toggle("hidden", !opening);
  if (opening) await loadHistory(true);
});
$("btn-history-close").addEventListener("click", () => $("history-pane").classList.add("hidden"));
$("btn-history-refresh").addEventListener("click", () => loadHistory(true));
$("btn-history-more").addEventListener("click", () => loadHistory(false));

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
  $("cfg-ls-pass").value = ls.password; // get_log_sync 掩码返回 "********",空 = 未设置
  $("cfg-ls-pass").placeholder = ls.password ? "留空 = 不修改" : "password";
  $("ls-dir").value = ls.remote_dir || "";
  $("ls-interval").value = ls.sync_interval_minutes || 5;
  setMsg("ls-msg", ls.enabled ? "已启用" : "已关闭", ls.enabled ? "ok" : "");
}

// ---------- 远程日志自动保存(无保存按钮) ----------
async function autoSaveLogSync() {
  try {
    await invoke("set_log_sync", {
      enabled: $("ls-enabled").checked,
      baseUrl: $("ls-url").value.trim(),
      username: $("cfg-ls-user").value.trim(),
      password: $("cfg-ls-pass").value,
      remoteDir: $("ls-dir").value.trim(),
      syncIntervalMinutes: parseInt($("ls-interval").value) || 5,
    });
    setMsg("ls-msg", "已自动保存 ✓", "ok");
  } catch (e) {
    setMsg("ls-msg", e, "err");
  }
}
["ls-url", "ls-dir", "cfg-ls-user", "cfg-ls-pass", "ls-interval", "ls-enabled"].forEach(id => $(id).addEventListener("change", autoSaveLogSync));

// ---------- 上传偏好自动保存 ----------
async function saveUploadPrefs() {
  try {
    await invoke("set_upload_prefs", {
      concurrency: parseInt($("up-concurrency").value) || 3,
      maxRetries: parseInt($("up-retries").value) || 3,
    });
    setMsg("up-msg", "已自动保存 ✓(并发数下次「连接」后生效)", "ok");
  } catch (e) {
    setMsg("up-msg", e, "err");
  }
}
["up-concurrency", "up-retries"].forEach(id => $(id).addEventListener("change", saveUploadPrefs));

// ---------- 通用偏好自动保存 ----------
async function saveGeneralPrefs() {
  try {
    await invoke("set_general_prefs", {
      autostart: $("gen-autostart").checked,
      silentStart: $("gen-silent").checked,
      minimizeOnClose: $("gen-minimize").checked,
      checkUpdateOnStart: $("gen-checkupdate").checked,
    });
    setMsg("gen-msg", "已自动保存 ✓", "ok");
  } catch (e) {
    setMsg("gen-msg", e, "err");
  }
}
["gen-autostart", "gen-silent", "gen-minimize", "gen-checkupdate"].forEach(id => $(id).addEventListener("change", saveGeneralPrefs));

// ---------- 密码框显示/隐藏 ----------
// 常规回显是掩码 "********";点「显示」时若内容仍是掩码,则向后端取真实密码(reveal_password);
// 若是用户正在输入的新密码则仅切换可见性。收起时恢复掩码回显。
document.querySelectorAll(".pw-toggle").forEach(btn => {
  btn.addEventListener("click", async () => {
    const input = $(btn.dataset.target);
    const show = input.type === "password";
    input.type = show ? "text" : "password";
    btn.textContent = show ? "隐藏" : "显示";
    if (show && input.value === "********") {
      const kind = input.id === "cfg-pass" ? "main" : "logsync";
      try {
        input.value = await invoke("reveal_password", { kind });
        input.dataset.revealed = "1";
      } catch (e) { console.warn(e); }
    } else if (!show && input.dataset.revealed) {
      input.value = "********";
      delete input.dataset.revealed;
    }
  });
});
// 密码框掩码占位:聚焦全选,输入即替换;清空后保存 = 沿用已存密码(后端以 "********" 为哨兵)
["cfg-pass", "cfg-ls-pass"].forEach(id => $(id).addEventListener("focus", e => e.target.select()));

// ---------- test log sync channel (no save) ----------
async function testLogSync() {
  setMsg("ls-msg", "测试中…");
  try {
    const r = await invoke("test_log_sync", {
      baseUrl: $("ls-url").value.trim(),
      username: $("cfg-ls-user").value.trim(),
      password: $("cfg-ls-pass").value,
      remoteDir: $("ls-dir").value.trim(),
    });
    setMsg("ls-msg", r.absoluteDir ? `日志通道可用 ✓ 实际落盘:${r.absoluteDir}` : `日志通道可用 ✓(远端目录 ${r.remoteDir}/ 已就绪)`, "ok");
  } catch (e) {
    setMsg("ls-msg", "日志通道不可用: " + e, "err");
  }
}
$("btn-ls-test").addEventListener("click", testLogSync);

// ---------- manual log upload (no timer, no enabled check) ----------
$("btn-ls-upload").addEventListener("click", async () => {
  const btn = $("btn-ls-upload");
  btn.disabled = true;
  setMsg("ls-msg", "日志上传中…");
  try {
    const r = await invoke("upload_logs_now");
    if (!r.count) {
      setMsg("ls-msg", "没有可上传的日志文件");
    } else if (r.absoluteDir) {
      setMsg("ls-msg", `已上传 ${r.count} 个日志文件 → ${r.absoluteDir}`, "ok");
    } else {
      setMsg("ls-msg", `已上传 ${r.count} 个日志文件到 ${r.remoteDir}/`, "ok");
    }
  } catch (e) {
    setMsg("ls-msg", "日志上传失败: " + e, "err");
  } finally {
    btn.disabled = false;
  }
});

// ---------- buttons ----------
$("btn-settings").addEventListener("click", openSettings);
$("btn-back").addEventListener("click", () => showView("main"));
$("btn-connect").addEventListener("click", connectNow);
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
  let bootConnectErr = "";
  try {
    const s = await invoke("load_settings");
    // 后端 serde 返回值字段是 snake_case(has_password),不是 hasPassword——Tauri 只映射入参
    if (s.base_url && s.has_password) await invoke("connect");
  } catch (e) {
    bootConnectErr = String(e);
  }
  await refreshStatus();
  if ($("status").classList.contains("connected")) await renderTree();
  else {
    await openSettings(); // 必须走 openSettings 回填表单,showView 会留空表单,保存一次就误清已存配置
    if (bootConnectErr) setMsg("cfg-msg", "自动连接失败: " + bootConnectErr, "err");
  }
  renderQueue(await invoke("get_queue"));
  // 启动时自动检查更新(可在通用设置关闭)
  try {
    const g = await invoke("get_general_prefs");
    if (g.check_update_on_start) {
      const u = await invoke("check_update").catch(() => null);
      if (u && u.available) setMsg("update-msg", `发现新版本 v${u.version},点「检查更新」安装`, "warn");
    }
  } catch (e) { console.warn(e); }
})();
