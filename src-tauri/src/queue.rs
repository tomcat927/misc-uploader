// queue.rs — upload queue state: SHA-256 content dedupe, history.json, helpers.
// Dedupe is content-level (user decision): same hash skips regardless of target folder.
// History covers only files uploaded by this app.
// 并发数/最大重试次数是用户设置(lib.rs UploadPrefs),RETRY_DELAYS_MS 为固定退避曲线。
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const RETRY_DELAYS_MS: [u64; 3] = [2000, 8000, 30000];
pub const HISTORY_MAX: usize = 10000;

#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    pub id: String,
    pub name: String,
    /// 本地绝对路径,下发前端用于「从哪来 → 到哪去」展示
    pub file_path: String,
    /// 拖入文件夹时该文件在拖入根下的相对路径(含根文件夹名);单文件拖入为 None
    #[serde(skip)]
    pub sub: Option<String>,
    pub size: u64,
    pub mtime: u64,
    pub sha: Option<String>,
    pub rel: Option<String>,
    pub state: String, // hashing | pending | processing | uploading | done | failed | skipped
    pub tries: u32,
    pub error: Option<String>,
    /// 完成/跳过/失败的打点时间(ms),前端「已完成」区块排序用
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub rel: String,
    pub time: u64,
}

/// 历史面板行(跨会话上传记录:远程路径 + 完成时间;按内容去重,同一内容记最新位置)
#[derive(Debug, Clone, Serialize)]
pub struct HistoryRow {
    pub rel: String,
    pub time: u64,
}

fn sort_history_rows(mut rows: Vec<(String, u64)>) -> Vec<(String, u64)> {
    // 完成时间倒序;同毫秒按路径稳定排序,保证分页确定
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows
}

pub struct Queue {
    pub items: Mutex<Vec<ArcItem>>,
    pub history: Mutex<HashMap<String, HistoryEntry>>,
    history_path: std::path::PathBuf,
    id_counter: AtomicU64,
}

pub type ArcItem = std::sync::Arc<Mutex<QueueItem>>;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn auto_dir_for(mtime_ms: u64) -> String {
    // mtime ms -> auto/YYYY/MM (local-time civil date, Hinnant algorithm, no chrono dep)
    let secs = (mtime_ms / 1000) as i64;
    let days = secs.div_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("auto/{y}/{m:02}")
}

impl Queue {
    pub fn new(history_path: std::path::PathBuf) -> Self {
        let history = std::fs::read(&history_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, HistoryEntry>>(&b).ok())
            .unwrap_or_default();
        Queue {
            items: Mutex::new(Vec::new()),
            history: Mutex::new(history),
            history_path,
            id_counter: AtomicU64::new(1),
        }
    }

    pub fn add(&self, files: Vec<(String, Option<String>, String, u64, u64)>) {
        // (file_path, sub_path, name, size, mtime)
        let mut items = self.items.lock().unwrap();
        for (file_path, sub, name, size, mtime) in files {
            let id = format!("u{}", self.id_counter.fetch_add(1, Ordering::Relaxed));
            items.push(Arc::new(Mutex::new(QueueItem {
                id,
                name,
                file_path,
                sub,
                size,
                mtime,
                sha: None,
                rel: None,
                state: "hashing".into(),
                tries: 0,
                error: None,
                finished_at: None,
            })));
        }
    }

    // atomically claim next work item; sets state to "processing"
    pub fn take_next(&self) -> Option<ArcItem> {
        let items = self.items.lock().unwrap();
        for it in items.iter() {
            let mut g = it.lock().unwrap();
            if g.state == "hashing" || g.state == "pending" {
                g.state = "processing".into();
                return Some(it.clone());
            }
        }
        None
    }

    pub fn retry_failed(&self) {
        let items = self.items.lock().unwrap();
        for it in items.iter() {
            let mut g = it.lock().unwrap();
            if g.state == "failed" {
                g.state = "hashing".to_string();
                g.tries = 0;
                g.error = None;
                g.sha = None;
                g.finished_at = None; // 重新入队,回到「进行中」区块
            }
        }
    }

    pub fn clear_finished(&self) {
        let mut items = self.items.lock().unwrap();
        items.retain(|it| {
            let s = it.lock().unwrap().state.clone();
            matches!(s.as_str(), "hashing" | "pending" | "processing" | "uploading")
        });
    }

    pub fn snapshot(&self) -> Vec<QueueItem> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .map(|it| it.lock().unwrap().clone())
            .collect()
    }

    pub fn history_contains(&self, sha: &str) -> Option<HistoryEntry> {
        self.history.lock().unwrap().get(sha).cloned()
    }

    // 内存历史按完成时间倒序(历史面板数据源;与磁盘同步落盘,两者等价)
    pub fn history_rows(&self) -> Vec<(String, u64)> {
        let hist = self.history.lock().unwrap();
        sort_history_rows(hist.values().map(|e| (e.rel.clone(), e.time)).collect())
    }

    pub fn history_put(&self, sha: String, entry: HistoryEntry) {
        {
            let mut hist = self.history.lock().unwrap();
            hist.insert(sha, entry);
            if hist.len() > HISTORY_MAX {
                let mut entries: Vec<(&String, &HistoryEntry)> = hist.iter().collect();
                entries.sort_by_key(|(_, v)| v.time);
                let drop_n = hist.len() - HISTORY_MAX;
                let drop: std::collections::HashSet<String> = entries
                    .iter()
                    .take(drop_n)
                    .map(|(k, _)| (*k).clone())
                    .collect();
                hist.retain(|k, _| !drop.contains(k));
            }
        }
        let hist = self.history.lock().unwrap();
        let _ = std::fs::write(&self.history_path, serde_json::to_vec(&*hist).unwrap_or_default());
    }
}

// 未连接时(内存队列不存在)从磁盘读历史,历史面板跨会话可用
pub fn load_history_rows(path: &std::path::Path) -> Vec<(String, u64)> {
    let map: HashMap<String, HistoryEntry> = match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
        Err(_) => return Vec::new(),
    };
    sort_history_rows(map.into_values().map(|e| (e.rel, e.time)).collect())
}
