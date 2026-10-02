// log.rs — app logging: local append file (logs/applog-YYYYMMDD.log, China time) +
// periodic sync of all log files to the repo's logs/ dir via WebDAV (remote diagnostics).
use crate::openlist::OpenListClient;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<AppLog> = OnceLock::new();

pub struct AppLog {
    dir: PathBuf,
}

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

pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let _ = LOG.set(AppLog { dir });
    log("app log initialized");
}

pub fn log(msg: &str) {
    let line = format!("[{}] {}", bj_stamp(), msg);
    if let Some(l) = LOG.get() {
        let path = l.dir.join(format!("applog-{}.log", bj_date_compact()));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }
    println!("{line}");
}

pub fn logf(args: std::fmt::Arguments) {
    log(&format!("{args}"));
}

// sync all local log files (small) to remote logs/ — full overwrite per file name
pub async fn sync_remote(client: &OpenListClient) -> Result<(), String> {
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
    client.mkdirp("logs").await?;
    for (path, name) in files {
        client
            .put_file(&format!("logs/{name}"), path.to_string_lossy().as_ref())
            .await?;
    }
    Ok(())
}
