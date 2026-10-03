// log.rs — app logging: local append file (logs/applog-YYYYMMDD.log, China time) +
// periodic sync to remote logs/ via WebDAV using a DEDICATED logger account
// (credentials injected at compile time in CI; NOT encrypted, NOT in the misc hot layer).
use crate::openlist::OpenListClient;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static LOG: OnceLock<AppLog> = OnceLock::new();
static SYNC_DISABLED_LOGGED: AtomicBool = AtomicBool::new(false);

pub struct AppLog {
    dir: PathBuf,
    version: String,
}

const APP_NAME: &str = "misc-uploader";
// remote path (relative to logger account base_path): {appName}/logs/
const REMOTE_DIR: &str = "misc-uploader/logs";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// China time (UTC+8) civil parts from unix ms. Hinnant civil-from-days.
fn bj_parts(ms: u64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = (ms / 1000) as i64 + 8 * 3600;
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
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
    let d = doy - (153 * mp + 2) / 5 + 1;
    let h = (sod / 3600) as u32;
    let mi = ((sod % 3600) / 60) as u32;
    let s = (sod % 60) as u32;
    (y, m as u32, d as u32, h, mi, s)
}

pub fn bj_stamp() -> String {
    let (y, mo, d, h, mi, s) = bj_parts(now_ms());
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

fn bj_date_compact() -> String {
    let (y, mo, d, _, _, _) = bj_parts(now_ms());
    format!("{y:04}{mo:02}{d:02}")
}

pub fn init(dir: PathBuf, version: &str) {
    let _ = std::fs::create_dir_all(&dir);
    let _ = LOG.set(AppLog { dir: dir, version: version.to_string() });
    log(&format!("app v{} started", version));
}

pub fn log(msg: &str) {
    let line = format!("[{} v{}] {}", bj_stamp(), LOG.get().map(|l| l.version.as_str()).unwrap_or("?"), msg);
    if let Some(l) = LOG.get() {
        let path = l.dir.join(format!("applog-{}.log", bj_date_compact()));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }
    println!("{line}");
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoteInfo {
    pub enabled: bool,
    pub base_url: Option<String>,
    pub user: Option<String>,
    pub remote_dir: String,
}

pub fn remote_info() -> RemoteInfo {
    let (base, user) = (option_env!("LOG_BASE_URL"), option_env!("LOG_USER"));
    RemoteInfo {
        enabled: base.is_some() && user.is_some(),
        base_url: base.map(String::from),
        user: user.map(String::from),
        remote_dir: REMOTE_DIR.into(),
    }
}

// logger account client, credentials injected at compile time by CI (repo secrets).
// built without them (local dev) -> remote sync disabled.
fn logger_client() -> Option<OpenListClient> {
    let base = option_env!("LOG_BASE_URL")?;
    let user = option_env!("LOG_USER")?;
    let pass = option_env!("LOG_PASS")?;
    Some(OpenListClient::new(base, user, pass))
}

// sync all local log files to remote {APP_NAME}/logs/ — full overwrite per file name
pub async fn sync_remote() -> Result<(), String> {
    let Some(client) = logger_client() else {
        if !SYNC_DISABLED_LOGGED.swap(true, Ordering::Relaxed) {
            log("remote log sync disabled (built without LOG_* env)");
        }
        return Ok(());
    };
    let Some(l) = LOG.get() else {
        return Err("log not initialized".into());
    };
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&l.dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("applog-") && name.ends_with(".log") {
                files.push((e.path(), name));
            }
        }
    }
    if files.is_empty() {
        return Ok(());
    }
    client.login().await?;
    client
        .mkdirp(&format!("{APP_NAME}/logs"))
        .await
        .map_err(|e| format!("mkdir: {e}"))?;
    for (path, name) in files {
        let remote = format!("{REMOTE_DIR}/{name}");
        client
            .put_file(&remote, path.to_string_lossy().as_ref())
            .await
            .map_err(|e| format!("put {name}: {e}"))?;
    }
    Ok(())
}
