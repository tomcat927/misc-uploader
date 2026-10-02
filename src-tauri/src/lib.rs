// lib.rs — Tauri app: commands, drag&drop intake, upload workers.
mod openlist;
mod queue;

use openlist::OpenListClient;
use queue::{Queue, QueueItem};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{DragDropEvent, Emitter, Manager, State, WindowEvent};

pub struct AppState {
    pub client: Mutex<Option<Arc<OpenListClient>>>,
    pub queue: Mutex<Option<Arc<Queue>>>,
    pub config: Mutex<Config>,
    pub mode: Mutex<String>,   // "manual" | "auto"
    pub target: Mutex<String>, // manual target dir, relative to base_path
    pub config_path: Mutex<Option<PathBuf>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub base_url: String,
    pub username: String,
    pub password: String,
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
    if let Some(q) = state.queue.lock().unwrap().as_ref() {
        let _ = app.emit("queue-updated", q.snapshot());
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

#[tauri::command]
async fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    base_url: String,
    username: String,
    password: String,
) -> Result<(), String> {
    let cur = state.config.lock().unwrap().clone();
    let cfg = Config {
        base_url: base_url.trim_end_matches('/').to_string(),
        username,
        password: if password.is_empty() { cur.password } else { password },
    };
    let client = do_connect(&cfg).await?;
    if let Some(p) = state.config_path.lock().unwrap().clone() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(&p, serde_json::to_vec_pretty(&cfg).unwrap_or_default());
    }
    start_session(&app, &state, Arc::new(client));
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
    for _ in 0..queue::CONCURRENCY {
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

// ---- drag & drop intake ----

fn collect_files(dir: &std::path::Path, out: &mut Vec<(String, String, u64, u64)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(&p, out);
            } else if let Ok(meta) = e.metadata() {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let mtime = modified_ms(&meta);
                out.push((p.to_string_lossy().into_owned(), name, meta.len(), mtime));
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
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                collect_files(&p, &mut files);
            } else if let Ok(meta) = std::fs::metadata(&p) {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                files.push((p.to_string_lossy().into_owned(), name, meta.len(), modified_ms(&meta)));
            }
        }
        if let Some(q) = state.queue.lock().unwrap().as_ref() {
            if !files.is_empty() {
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
            drop(g);
            emit_queue(app);
            return;
        }
        // 3. resolve rel path
        if g.rel.is_none() {
            let (mode, target) = current_mode(app);
            let name = g.name.clone();
            let mtime = g.mtime;
            g.rel = Some(if mode == "auto" {
                format!("{}/{}", queue::auto_dir_for(mtime), name)
            } else {
                join_rel(&target, &name)
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
    let dir = std::path::Path::new(&rel)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if !dir.is_empty() && dir != "." {
        if let Err(e) = client.mkdirp(&dir).await {
            retry_or_fail(app, q, &item, format!("mkdir: {e}"));
            emit_queue(app);
            return;
        }
    }
    match client.put_file(&rel, &file_path).await {
        Ok(_) => {
            let mut g = item.lock().unwrap();
            g.state = "done".into();
            let sha = g.sha.clone().unwrap_or_default();
            q.history_put(sha, queue::HistoryEntry { rel, time: queue::now_ms() });
            drop(g);
        }
        Err(e) => {
            retry_or_fail(app, q, &item, e);
        }
    }
    emit_queue(app);
}

fn retry_or_fail(app: &tauri::AppHandle, item: &queue::ArcItem, err: String) {
    let (tries, state) = {
        let mut g = item.lock().unwrap();
        g.tries += 1;
        g.error = Some(err);
        if g.tries > queue::RETRIES {
            g.state = "failed".into();
        } else {
            g.state = "cooldown".into();
        }
        (g.tries, g.state.clone())
    };
    if state == "cooldown" {
        let delay = queue::RETRY_DELAYS_MS[(tries - 1).min(2) as usize];
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
    (
        state.mode.lock().unwrap().clone(),
        state.target.lock().unwrap().clone(),
    )
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
        .setup(|app| {
            let dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let _ = std::fs::create_dir_all(&dir);
            let cfg_path = dir.join("config.json");
            let cfg: Config = std::fs::read(&cfg_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Config {
                    base_url: String::new(),
                    username: String::new(),
                    password: String::new(),
                });
            app.manage(AppState {
                client: Mutex::new(None),
                queue: Mutex::new(None),
                config: Mutex::new(cfg),
                mode: Mutex::new("manual".into()),
                target: Mutex::new(String::new()),
                config_path: Mutex::new(Some(cfg_path)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            connect,
            list_dir,
            set_target,
            get_queue,
            retry_failed,
            clear_finished
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
