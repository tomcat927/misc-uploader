// lib.rs — Tauri app: commands, drag&drop intake, upload workers,
// hot-update (tauri-plugin-updater), remote log sync.
mod log;
mod openlist;
mod queue;

use openlist::OpenListClient;
use queue::{Queue, QueueItem};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tauri::{DragDropEvent, Emitter, Manager, State, WindowEvent};

pub struct AppState {
    pub client: Mutex<Option<Arc<OpenListClient>>>,
    pub queue: Mutex<Option<Arc<Queue>>>,
    pub config: Mutex<Config>,
    pub mode: Mutex<String>,   // "manual" | "auto"
    pub target: Mutex<String>, // manual target dir, relative to base_path
    pub config_path: Mutex<Option<PathBuf>>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LogSyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 存放目录(WebDAV 相对路径,相对 logger 账号 base_path),设置页可改
    #[serde(default = "default_remote_dir")]
    pub remote_dir: String,
    /// 定期上传间隔(分钟),设置页可改
    #[serde(default = "default_sync_interval")]
    pub sync_interval_minutes: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UploadPrefs {
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

impl Default for UploadPrefs {
    fn default() -> Self {
        UploadPrefs { concurrency: default_concurrency(), max_retries: default_max_retries() }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeneralPrefs {
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub silent_start: bool,
    #[serde(default = "default_true")]
    pub minimize_on_close: bool,
    #[serde(default = "default_true")]
    pub check_update_on_start: bool,
}

impl Default for GeneralPrefs {
    fn default() -> Self {
        GeneralPrefs {
            autostart: false,
            silent_start: false,
            minimize_on_close: default_true(),
            check_update_on_start: default_true(),
        }
    }
}

fn default_remote_dir() -> String {
    "本地磁盘/misc-uploader/logs".into()
}
fn default_sync_interval() -> u32 {
    5
}
fn default_concurrency() -> u32 {
    3
}
fn default_max_retries() -> u32 {
    3
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub base_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub log_sync: LogSyncConfig,
    #[serde(default)]
    pub upload: UploadPrefs,
    #[serde(default)]
    pub general: GeneralPrefs,
}

#[derive(Debug, Clone, Serialize)]
struct SettingsView {
    base_url: String,
    username: String,
    has_password: bool,
    connected: bool,
}

fn emit_queue(app: &tauri::AppHandle) {
    let state: State<AppState> = app.state();
    let snapshot = state
        .queue
        .lock()
        .unwrap()
        .as_ref()
        .map(|q| q.snapshot());
    if let Some(items) = snapshot {
        let _ = app.emit("queue-updated", items);
    }
}

// ---- commands ----

#[tauri::command]
fn load_settings(state: State<AppState>) -> SettingsView {
    let cfg = state.config.lock().unwrap().clone();
    SettingsView {
        base_url: cfg.base_url,
        username: cfg.username,
        has_password: !cfg.password.is_empty(),
        connected: state.client.lock().unwrap().is_some(),
    }
}

// 密码「显示」按钮点击时按需下发真实密码(修订原「真实密码不下发前端」拍板,2026-10-03):
// 密码本就明文存本机 config.json(见 DESIGN.md 密码体系),UI 按需展示不增加暴露面;常规回显仍是掩码。
#[tauri::command]
fn reveal_password(state: State<AppState>, kind: String) -> Result<String, String> {
    let cfg = state.config.lock().unwrap();
    match kind.as_str() {
        "main" => Ok(cfg.password.clone()),
        "logsync" => Ok(cfg.log_sync.password.clone()),
        _ => Err("unknown password kind".into()),
    }
}

// 纯落盘,不碰网络(连接是独立动作)。设置页无保存按钮,字段失焦即调用。
#[tauri::command]
fn save_settings(
    state: State<AppState>,
    base_url: String,
    username: String,
    password: String,
) -> Result<(), String> {
    let cur = state.config.lock().unwrap().clone();
    let cfg = Config {
        base_url: base_url.trim_end_matches('/').to_string(),
        username,
        password: if password.is_empty() || password == "********" { cur.password } else { password },
        log_sync: cur.log_sync,
        upload: cur.upload,
        general: cur.general,
    };
    // 空 url/用户名的保存一律拒绝且不落盘,防止把已存配置覆盖成空(密码/日志同步设置同理不丢)
    if cfg.base_url.is_empty() || cfg.username.is_empty() {
        return Err("服务器地址和用户名必填(密码留空 = 沿用已保存)".into());
    }
    if let Some(p) = state.config_path.lock().unwrap().clone() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(&p, serde_json::to_vec_pretty(&cfg).unwrap_or_default());
    }
    *state.config.lock().unwrap() = cfg;
    Ok(())
}

#[tauri::command]
async fn connect(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let cfg = state.config.lock().unwrap().clone();
    if cfg.base_url.is_empty() || cfg.username.is_empty() || cfg.password.is_empty() {
        return Err("server / username / password are all required — open settings".into());
    }
    let client = do_connect(&cfg).await?;
    start_session(&app, &state, Arc::new(client));
    Ok(())
}

async fn do_connect(cfg: &Config) -> Result<OpenListClient, String> {
    if cfg.base_url.is_empty() || cfg.username.is_empty() || cfg.password.is_empty() {
        return Err("server / username / password are all required".into());
    }
    let client = OpenListClient::new(&cfg.base_url, &cfg.username, &cfg.password);
    client.login().await?;
    Ok(client)
}

fn start_session(app: &tauri::AppHandle, state: &State<AppState>, client: Arc<OpenListClient>) {
    let history_path = state
        .config_path
        .lock()
        .unwrap()
        .clone()
        .map(|p| p.parent().unwrap().join("history.json"))
        .unwrap_or_else(|| PathBuf::from("history.json"));
    let q = Arc::new(Queue::new(history_path));
    *state.client.lock().unwrap() = Some(client.clone());
    *state.queue.lock().unwrap() = Some(q.clone());
    let (conc, max_retries) = {
        let cfg = state.config.lock().unwrap();
        (cfg.upload.concurrency.max(1) as usize, cfg.upload.max_retries)
    };
    for _ in 0..conc {
        let app = app.clone();
        let client = client.clone();
        let q = q.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match q.take_next() {
                    Some(item) => process_item(&app, &client, &q, item).await,
                    None => {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                }
            }
        });
    }
    // remote log sync: push local log files at a user-configured interval (only if enabled in settings)
    {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let interval_min = {
                    let st: State<AppState> = app2.state();
                    let cfg = st.config.lock().unwrap();
                    cfg.log_sync.sync_interval_minutes.max(1)
                };
                tokio::time::sleep(std::time::Duration::from_secs(interval_min as u64 * 60)).await;
                let ls = {
                    let st: State<AppState> = app2.state();
                    let cfg = st.config.lock().unwrap();
                    cfg.log_sync.clone()
                };
                if !ls.enabled || ls.base_url.is_empty() || ls.username.is_empty() || ls.password.is_empty() || ls.remote_dir.is_empty() {
                    continue;
                }
                if let Err(e) = log::sync_remote(&ls.base_url, &ls.username, &ls.password, &ls.remote_dir).await {
                    log::log(&format!("log sync: {e}"));
                }
            }
        });
    }
    log::log(&format!(
        "session started (user {}, concurrency {conc}, max retries {max_retries})",
        state.config.lock().unwrap().username
    ));
    emit_queue(app);
}

#[tauri::command]
async fn list_dir(state: State<'_, AppState>, path: String) -> Result<Vec<openlist::Entry>, String> {
    let client = state
        .client
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("not connected")?
        .clone();
    client.list(&path).await
}

#[tauri::command]
async fn new_dir(state: State<'_, AppState>, parent: String, name: String) -> Result<(), String> {
    let name = name.trim().trim_matches('/').to_string();
    if name.is_empty() {
        return Err("文件夹名称必填".into());
    }
    if name.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
        return Err("文件夹名称含非法字符".into());
    }
    let client = state.client.lock().unwrap().as_ref().ok_or("not connected")?.clone();
    let path = if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") };
    client.mkdirp(&path).await?;
    log::log(&format!("new dir created: {path}"));
    Ok(())
}

#[tauri::command]
fn set_target(state: State<AppState>, mode: String, target: String) {
    *state.mode.lock().unwrap() = mode;
    *state.target.lock().unwrap() = target;
}

#[tauri::command]
fn get_queue(state: State<AppState>) -> Vec<QueueItem> {
    state
        .queue
        .lock()
        .unwrap()
        .as_ref()
        .map(|q| q.snapshot())
        .unwrap_or_default()
}

#[tauri::command]
fn retry_failed(state: State<AppState>) {
    if let Some(q) = state.queue.lock().unwrap().as_ref() {
        q.retry_failed();
    }
}

#[tauri::command]
fn clear_finished(state: State<AppState>) {
    if let Some(q) = state.queue.lock().unwrap().as_ref() {
        q.clear_finished();
    }
}

#[tauri::command]
fn get_log_sync(state: State<AppState>) -> LogSyncConfig {
    let mut ls = state.config.lock().unwrap().log_sync.clone();
    if !ls.password.is_empty() { ls.password = "********".into(); }
    ls
}

#[tauri::command]
fn set_log_sync(state: State<AppState>, enabled: bool, base_url: String, username: String, password: String, remote_dir: String, sync_interval_minutes: u32) {
    let mut cfg = state.config.lock().unwrap();
    let cur = cfg.log_sync.clone();
    cfg.log_sync = LogSyncConfig {
        enabled,
        base_url: base_url.trim_end_matches('/').to_string(),
        username,
        password: if password.is_empty() || password == "********" { cur.password } else { password },
        remote_dir: remote_dir.trim().trim_matches('/').to_string(),
        sync_interval_minutes: sync_interval_minutes.clamp(1, 1440),
    };
    if let Some(p) = state.config_path.lock().unwrap().clone() {
        let _ = std::fs::write(&p, serde_json::to_vec_pretty(&*cfg).unwrap_or_default());
    }
    log::log(&format!("log sync config saved (enabled={enabled}, target {})", cfg.log_sync.base_url));
}

fn persist_config(state: &State<AppState>, cfg: &Config) {
    if let Some(p) = state.config_path.lock().unwrap().clone() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(&p, serde_json::to_vec_pretty(cfg).unwrap_or_default());
    }
}

#[tauri::command]
fn get_upload_prefs(state: State<AppState>) -> UploadPrefs {
    state.config.lock().unwrap().upload.clone()
}

#[tauri::command]
fn set_upload_prefs(state: State<AppState>, concurrency: u32, max_retries: u32) -> UploadPrefs {
    let mut cfg = state.config.lock().unwrap();
    cfg.upload.concurrency = concurrency.clamp(1, 16);
    cfg.upload.max_retries = max_retries.min(10);
    let prefs = cfg.upload.clone();
    persist_config(&state, &cfg);
    drop(cfg);
    log::log(&format!(
        "upload prefs saved (concurrency {}, max retries {})",
        prefs.concurrency, prefs.max_retries
    ));
    prefs
}

#[tauri::command]
fn get_general_prefs(state: State<AppState>) -> GeneralPrefs {
    state.config.lock().unwrap().general.clone()
}

#[tauri::command]
fn set_general_prefs(
    app: tauri::AppHandle,
    state: State<AppState>,
    autostart: bool,
    silent_start: bool,
    minimize_on_close: bool,
    check_update_on_start: bool,
) -> Result<GeneralPrefs, String> {
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.general = GeneralPrefs { autostart, silent_start, minimize_on_close, check_update_on_start };
        persist_config(&state, &cfg);
    }
    // 自启注册立即生效;先 disable 再 enable 强制刷新,防止升级后注册表残留旧 exe 路径(照抄 openlist-uploader)
    use tauri_plugin_autostart::ManagerExt;
    let m = app.autolaunch();
    let enabled_now = m.is_enabled().unwrap_or(false);
    if autostart {
        if enabled_now {
            let _ = m.disable();
        }
        m.enable().map_err(|e| format!("开机自启注册失败: {e}"))?;
        log::log("autostart enabled");
    } else if enabled_now {
        let _ = m.disable();
        log::log("autostart disabled");
    }
    let prefs = state.config.lock().unwrap().general.clone();
    Ok(prefs)
}

#[tauri::command]
async fn test_log_sync(
    state: State<'_, AppState>,
    base_url: String,
    username: String,
    password: String,
    remote_dir: String,
) -> Result<serde_json::Value, String> {
    let ls = state.config.lock().unwrap().log_sync.clone();
    let base_url = base_url.trim_end_matches('/').to_string();
    let password = if password.is_empty() || password == "********" { ls.password } else { password };
    let remote_dir = remote_dir.trim().trim_matches('/').to_string();
    if base_url.is_empty() || username.is_empty() || password.is_empty() {
        return Err("日志服务器 / 账号 / 密码 三项都必填(密码留空时使用已保存的密码)".into());
    }
    if remote_dir.is_empty() {
        return Err("日志存放目录必填".into());
    }
    let client = OpenListClient::new(&base_url, &username, &password);
    client.login().await?;
    // MKCOL 逐级幂等(已存在不算失败);验证账号对目标目录确实可写
    client
        .mkdirp(&remote_dir)
        .await
        .map_err(|e| format!("目标目录不可写: {e}"))?;
    log::log(&format!("log sync test OK: {username}@{base_url} -> {remote_dir}"));
    Ok(serde_json::json!({ "ok": true, "remoteDir": remote_dir }))
}

#[tauri::command]
fn open_log_dir(state: State<AppState>) {
    if let Some(p) = state.config_path.lock().unwrap().clone() {
        let logs = p.parent().unwrap().join("logs");
        let _ = std::fs::create_dir_all(&logs);
        let _ = std::process::Command::new("explorer").arg(logs).spawn();
    }
}

// ---- hot update ----

#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    log::log(&format!("update check: {}", update.is_some()));
    Ok(match update {
        Some(u) => serde_json::json!({
            "available": true,
            "version": u.version,
            "notes": u.body.clone().unwrap_or_default(),
        }),
        None => serde_json::json!({"available": false}),
    })
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update available")?;
    log::log(&format!("downloading update {}", update.version));
    let emit_app = app.clone();
    let mut on_chunk = move |chunk: usize, total: Option<u64>| {
        let _ = emit_app.emit("update-progress", serde_json::json!({
            "downloaded": chunk, "total": total,
        }));
    };
    update
        .download_and_install(&mut on_chunk, || {})
        .await
        .map_err(|e| e.to_string())?;
    log::log("update installed, restarting app");
    app.restart();
}

// ---- drag & drop intake ----

// (绝对路径, 拖入根下的相对路径(含根文件夹名,单文件 None), basename, size, mtime)
type IntakeFile = (String, Option<String>, String, u64, u64);

// 递归收集,保留文件夹内部结构:sub = 拖入根文件夹名/子路径/文件名(拍平会让不同子目录同名文件互相覆盖)
fn collect_files(root_name: &str, dir: &std::path::Path, prefix: &str, out: &mut Vec<IntakeFile>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let seg = e.file_name().to_string_lossy().replace('\\', "/");
            let sub = if prefix.is_empty() { format!("{root_name}/{seg}") } else { format!("{prefix}/{seg}") };
            if p.is_dir() {
                collect_files(root_name, &p, &sub, out);
            } else if let Ok(meta) = e.metadata() {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                out.push((p.to_string_lossy().into_owned(), Some(sub), name, meta.len(), modified_ms(&meta)));
            }
        }
    }
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn handle_drop(app: tauri::AppHandle, paths: Vec<std::path::PathBuf>) {
    tauri::async_runtime::spawn(async move {
        let state: State<AppState> = app.state();
        let connected = state.client.lock().unwrap().is_some();
        if !connected {
            return;
        }
        let mut files: Vec<IntakeFile> = Vec::new();
        for p in paths {
            if p.is_dir() {
                let root_name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|| "root".into());
                collect_files(&root_name, &p, "", &mut files);
            } else if let Ok(meta) = std::fs::metadata(&p) {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                files.push((p.to_string_lossy().into_owned(), None, name, meta.len(), modified_ms(&meta)));
            }
        }
        if let Some(q) = state.queue.lock().unwrap().as_ref() {
            if !files.is_empty() {
                log::log(&format!("drop: queued {} item(s)", files.len()));
                q.add(files);
            }
        }
        emit_queue(&app);
    });
}

// ---- workers ----

async fn process_item(
    app: &tauri::AppHandle,
    client: &OpenListClient,
    q: &Queue,
    item: queue::ArcItem,
) {
    // 1. hash (once)
    {
        let need_hash = item.lock().unwrap().sha.is_none();
        if need_hash {
            let fp = item.lock().unwrap().file_path.clone();
            match hash_file(&fp).await {
                Ok(h) => item.lock().unwrap().sha = Some(h),
                Err(e) => {
                    fail_item(&item, format!("hash: {e}"));
                    emit_queue(app);
                    return;
                }
            }
        }
    }
    // 2. content dedupe
    {
        let mut g = item.lock().unwrap();
        let sha = g.sha.clone().expect("sha set");
        if let Some(seen) = q.history_contains(&sha) {
            g.state = "skipped".into();
            g.rel = Some(seen.rel.clone());
            g.error = Some(format!("already in repo: {}", seen.rel));
            log::log(&format!("skip {}: {}", g.name, seen.rel));
            drop(g);
            emit_queue(app);
            return;
        }
        // 3. resolve rel path(拖入文件夹时保留内部结构:sub = 根文件夹名/子路径/文件名)
        if g.rel.is_none() {
            let (mode, target) = current_mode(app);
            let display_path = match g.sub.clone() {
                Some(s) => s,
                None => g.name.clone(),
            };
            let mtime = g.mtime;
            g.rel = Some(if mode == "auto" {
                format!("{}/{}", queue::auto_dir_for(mtime), display_path)
            } else {
                join_rel(&target, &display_path)
            });
        }
        g.state = "uploading".into();
    }
    emit_queue(app);

    // 4. mkdir + upload
    let (rel, file_path) = {
        let g = item.lock().unwrap();
        (g.rel.clone().unwrap(), g.file_path.clone())
    };
    log::log(&format!("upload start: {rel}"));
    let dir = std::path::Path::new(&rel)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if !dir.is_empty() && dir != "." {
        if let Err(e) = client.mkdirp(&dir).await {
            retry_or_fail(app, &item, format!("mkdir: {e}"));
            emit_queue(app);
            return;
        }
    }
    let result = client.put_file(&rel, &file_path).await;
    let uploaded_ok = result.is_ok();
    match result {
        Ok(_) => {
            {
                let mut g = item.lock().unwrap();
                g.state = "done".into();
                let sha = g.sha.clone().unwrap_or_default();
                q.history_put(sha, queue::HistoryEntry { rel: rel.clone(), time: queue::now_ms() });
            }
            log::log(&format!("upload done: {rel}"));
        }
        Err(e) => {
            log::log(&format!("upload error {rel}: {e}"));
            retry_or_fail(app, &item, e);
        }
    }
    // 刷新目标目录缓存,触发 OpenList 增量索引(便于搜索新文件;尽力而为,失败不影响上传结果)
    // 注意:必须在 MutexGuard 作用域之外 await(guard 非 Send)
    if uploaded_ok && !dir.is_empty() && dir != "." {
        if let Err(e) = client.refresh_dir(&dir).await {
            log::log(&format!("refresh dir {dir} failed (不影响上传): {e}"));
        }
    }
    emit_queue(app);
}

fn retry_or_fail(app: &tauri::AppHandle, item: &queue::ArcItem, err: String) {
    let max_retries = {
        let st: State<AppState> = app.state();
        let cfg = st.config.lock().unwrap();
        cfg.upload.max_retries
    };
    let (tries, state) = {
        let mut g = item.lock().unwrap();
        g.tries += 1;
        g.error = Some(err);
        if g.tries > max_retries {
            g.state = "failed".into();
        } else {
            g.state = "cooldown".into();
        }
        (g.tries, g.state.clone())
    };
    if state == "cooldown" {
        let last = queue::RETRY_DELAYS_MS.len() as u32 - 1;
        let delay = queue::RETRY_DELAYS_MS[(tries - 1).min(last) as usize];
        let item = item.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            item.lock().unwrap().state = "pending".into();
        });
    }
    let _ = app;
}

fn fail_item(item: &queue::ArcItem, err: String) {
    let mut g = item.lock().unwrap();
    g.state = "failed".into();
    g.error = Some(err);
}

fn current_mode(app: &tauri::AppHandle) -> (String, String) {
    let state: State<AppState> = app.state();
    let mode = state.mode.lock().unwrap().clone();
    let target = state.target.lock().unwrap().clone();
    (mode, target)
}

fn join_rel(dir: &str, name: &str) -> String {
    let d = dir.trim_matches('/');
    if d.is_empty() {
        name.to_string()
    } else {
        format!("{d}/{name}")
    }
}

async fn hash_file(p: &str) -> Result<String, String> {
    let mut f = tokio::fs::File::open(p).await.map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

// ---- entry ----

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .setup(|app| {
            let dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let _ = std::fs::create_dir_all(&dir);
            log::init(dir.join("logs"), &app.package_info().version.to_string());
            let cfg_path = dir.join("config.json");
            let cfg: Config = std::fs::read(&cfg_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Config {
                    base_url: String::new(),
                    username: String::new(),
                    password: String::new(),
                    log_sync: Default::default(),
                    upload: Default::default(),
                    general: Default::default(),
                });
            app.manage(AppState {
                client: Mutex::new(None),
                queue: Mutex::new(None),
                config: Mutex::new(cfg),
                mode: Mutex::new("manual".into()),
                target: Mutex::new(String::new()),
                config_path: Mutex::new(Some(cfg_path)),
            });
            // 开机自启注册与配置同步(先 disable 再 enable 强制刷新,防注册表残留旧 exe 路径)
            {
                use tauri_plugin_autostart::ManagerExt;
                let want = {
                    let st = app.state::<AppState>();
                    let cfg = st.config.lock().unwrap();
                    cfg.general.autostart
                };
                let m = app.autolaunch();
                let is = m.is_enabled().unwrap_or(false);
                if want && !is {
                    let _ = m.enable();
                }
                if !want && is {
                    let _ = m.disable();
                }
            }
            // 静默启动:配置勾选,或以开机自启参数(--autostart)启动时隐藏主窗口,从托盘唤出
            let autostarted = std::env::args().any(|arg| arg == "--autostart");
            let silent = autostarted
                || {
                    let st = app.state::<AppState>();
                    let cfg = st.config.lock().unwrap();
                    cfg.general.silent_start
                };
            if silent {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            log::log(&format!("startup: silent={silent} autostarted={autostarted}"));
            // 托盘:左键/菜单唤出主窗口;关闭行为见 on_window_event(minimize_on_close)
            {
                use tauri::menu::{Menu, MenuItem};
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
                let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &quit])?;
                TrayIconBuilder::with_id("main-tray")
                    .icon(app.default_window_icon().expect("app icon").clone())
                    .tooltip("misc-uploader")
                    .menu(&menu)
                    .on_menu_event(|handle, event| match event.id.as_ref() {
                        "show" => {
                            if let Some(w) = handle.get_webview_window("main") {
                                let _ = w.unminimize();
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => handle.exit(0),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            if let Some(w) = tray.app_handle().get_webview_window("main") {
                                let _ = w.unminimize();
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    })
                    .build(app)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let minimize = {
                    let st = window.app_handle().state::<AppState>();
                    let cfg = st.config.lock().unwrap();
                    cfg.general.minimize_on_close
                };
                if minimize {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            reveal_password,
            connect,
            list_dir,
            new_dir,
            set_target,
            get_queue,
            retry_failed,
            clear_finished,
            get_log_sync,
            set_log_sync,
            test_log_sync,
            get_upload_prefs,
            set_upload_prefs,
            get_general_prefs,
            set_general_prefs,
            open_log_dir,
            check_update,
            install_update
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::WindowEvent {
                event: WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }),
                ..
            } = event
            {
                handle_drop(app_handle.clone(), paths.clone());
            }
        });
}
