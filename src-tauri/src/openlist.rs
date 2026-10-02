// openlist.rs — OpenList client: REST (login/list) + WebDAV (PUT/MKCOL). rustls HTTPS.
use crate::log;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::RwLock;
use std::time::Instant;
use tokio_util::io::ReaderStream;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Deserialize)]
struct ApiResp<T> {
    code: i32,
    message: Option<String>,
    data: Option<T>,
}

#[derive(Deserialize)]
struct ListData {
    #[serde(default)]
    content: Option<Vec<ListEntry>>,
}

#[derive(Deserialize)]
struct ListEntry {
    name: String,
    is_dir: bool,
    size: u64,
}

pub struct OpenListClient {
    base_url: String,
    http: Client,
    token: RwLock<Option<String>>,
    username: String,
    password: String,
}

fn enc_path(p: &str) -> String {
    p.split('/')
        .map(|seg| urlencoding::encode(seg).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

impl OpenListClient {
    pub fn new(base_url: &str, username: &str, password: &str) -> Self {
        OpenListClient {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: Client::builder()
                .build()
                .expect("reqwest client"),
            token: RwLock::new(None),
            username: username.to_string(),
            password: password.to_string(),
        }
    }

    pub async fn login(&self) -> Result<String, String> {
        let resp: ApiResp<serde_json::Value> = self
            .http
            .post(format!("{}/api/auth/login", self.base_url))
            .json(&json!({"username": self.username, "password": self.password}))
            .send()
            .await
            .map_err(|e| format!("login request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("login parse: {e}"))?;
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| "login failed".into());
            log::log(&format!("login FAILED: {m}"));
            return Err(m);
        }
        let token = resp
            .data
            .and_then(|d| d.get("token").and_then(|t| t.as_str()).map(String::from))
            .ok_or("login: no token in response")?;
        *self.token.write().unwrap() = Some(token.clone());
        log::log(&format!("login OK (user {})", self.username));
        Ok(token)
    }

    pub async fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        let token = self.token.read().unwrap().clone().ok_or("not logged in")?;
        let resp: ApiResp<ListData> = self
            .http
            .post(format!("{}/api/fs/list", self.base_url))
            .header("Authorization", &token)
            .json(&json!({"path": path, "page": 1, "per_page": 1000, "refresh": false}))
            .send()
            .await
            .map_err(|e| format!("list request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("list parse: {e}"))?;
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| format!("list {path} failed"));
            log::log(&format!("list {path} FAILED: {m}"));
            return Err(m);
        }
        let n = resp.data.as_ref().and_then(|d| d.content.as_ref()).map(|c| c.len()).unwrap_or(0);
        log::log(&format!("list {path}: {n} entries"));
        Ok(resp
            .data
            .and_then(|d| d.content)
            .unwrap_or_default()
            .into_iter()
            .map(|e| Entry { name: e.name, is_dir: e.is_dir, size: e.size })
            .collect())
    }

    // MKCOL each level; 405/301/200 mean "already there"
    pub async fn mkdirp(&self, rel_dir: &str) -> Result<(), String> {
        let mut cur = String::new();
        for seg in rel_dir.split('/').filter(|s| !s.is_empty()) {
            cur = if cur.is_empty() { seg.to_string() } else { format!("{cur}/{seg}") };
            let url = format!("{}/dav/{}", self.base_url, enc_path(&cur));
            let res = self
                .http
                .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &url)
                .header("Authorization", basic_auth(&self.username, &self.password))
                .send()
                .await
                .map_err(|e| format!("MKCOL {cur}: {e}"))?;
            let st = res.status().as_u16();
            if st != 201 && st != 200 && st != 405 && st != 301 {
                log::log(&format!("MKCOL {cur} FAILED -> {st}"));
                return Err(format!("MKCOL {cur} -> {st}"));
            }
            log::log(&format!("MKCOL {cur} -> {st}"));
        }
        Ok(())
    }

    pub async fn put_file(&self, rel: &str, file_path: &str) -> Result<u64, String> {
        let meta = tokio::fs::metadata(file_path)
            .await
            .map_err(|e| format!("stat {file_path}: {e}"))?;
        let total = meta.len();
        let file = tokio::fs::File::open(file_path)
            .await
            .map_err(|e| format!("open {file_path}: {e}"))?;
        let stream = ReaderStream::with_capacity(file, 256 * 1024);
        let url = format!("{}/dav/{}", self.base_url, enc_path(rel));
        let res = self
            .http
            .put(&url)
            .header("Authorization", basic_auth(&self.username, &self.password))
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", total.to_string())
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| format!("PUT {rel}: {e}"))?;
        let st = res.status().as_u16();
        if !(200..300).contains(&st) {
            let t = res.text().await.unwrap_or_default();
            log::log(&format!("PUT {rel} FAILED -> {st}"));
            return Err(format!("PUT {rel} -> {st} {}", t.chars().take(120).collect::<String>()));
        }
        log::log(&format!("PUT {rel}: complete ({} bytes, {:.1}s)", total, t0.elapsed().as_secs_f64()));
        Ok(total)
    }
}

fn basic_auth(user: &str, pass: &str) -> String {
    use std::fmt::Write;
    let mut out = String::from("Basic ");
    let raw = format!("{user}:{pass}");
    // base64 without external crate
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = raw.as_bytes();
    for chunk in b.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | (chunk.get(2).copied().unwrap_or(0) as u32);
        let _ = write!(out, "{}{}{}{}",
            T[(n >> 18) as usize & 63], T[(n >> 12) as usize & 63],
            if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' },
            if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
