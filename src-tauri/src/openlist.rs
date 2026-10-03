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
        let url = format!("{}/api/auth/login", self.base_url);
        // 协议日志:url/用户/响应状态全记;密码与 token 按凭据规约永不落日志
        log::log(&format!("login request: POST {url} (user {}, password ***)", self.username));
        let http_resp = self
            .http
            .post(&url)
            .json(&json!({"username": self.username, "password": self.password}))
            .send()
            .await;
        let http_resp = match http_resp {
            Ok(r) => r,
            Err(e) => {
                log::log(&format!("login network error: {e}"));
                return Err(format!("login request: {e}"));
            }
        };
        let status = http_resp.status();
        let text = http_resp.text().await.map_err(|e| format!("login read: {e}"))?;
        let resp: ApiResp<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                let snippet: String = text.chars().take(200).collect();
                log::log(&format!("login parse error: http {status}, body: {snippet}"));
                return Err(format!("login parse: {e}"));
            }
        };
        log::log(&format!(
            "login response: http {status} code {} message {}",
            resp.code,
            resp.message.clone().unwrap_or_default()
        ));
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| "login failed".into());
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
        if self.token.read().unwrap().is_none() {
            self.login().await?;
        }
        let (code, message, data) = self.list_request(path).await?;
        // token 会话过期:自动重登一次再试(参照 openlist-uploader 的 401 重登逻辑)
        let (code, message, data) = if code == 401 {
            log::log(&format!("list {path}: token expired, re-login once"));
            self.login().await?;
            self.list_request(path).await?
        } else {
            (code, message, data)
        };
        if code != 200 {
            let m = message.unwrap_or_else(|| format!("list {path} failed"));
            log::log(&format!("list {path} FAILED: {m}"));
            return Err(m);
        }
        let entries = data.and_then(|d| d.content).unwrap_or_default();
        log::log(&format!("list {path}: {} entries", entries.len()));
        Ok(entries
            .into_iter()
            .map(|e| Entry { name: e.name, is_dir: e.is_dir, size: e.size })
            .collect())
    }

    // 单次列目录请求,返回原始 (code, message, data),401 重试判定由调用方做
    async fn list_request(&self, path: &str) -> Result<(i32, Option<String>, Option<ListData>), String> {
        let token = self.token.read().unwrap().clone().ok_or("not logged in")?;
        let url = format!("{}/api/fs/list", self.base_url);
        log::log(&format!("list request: POST {url} path={path}"));
        let resp: ApiResp<ListData> = self
            .http
            .post(&url)
            .header("Authorization", &token)
            .json(&json!({"path": path, "page": 1, "per_page": 1000, "refresh": false}))
            .send()
            .await
            .map_err(|e| {
                log::log(&format!("list network error: {e}"));
                format!("list request: {e}")
            })?
            .json()
            .await
            .map_err(|e| format!("list parse: {e}"))?;
        Ok((resp.code, resp.message, resp.data))
    }

    // 强制刷新目录(list refresh=true 穿透缓存),触发 OpenList 增量索引更新 —— 上传成功后调用
    pub async fn refresh_dir(&self, path: &str) -> Result<(), String> {
        let token = self.token.read().unwrap().clone().ok_or("not logged in")?;
        let resp: ApiResp<serde_json::Value> = self
            .http
            .post(format!("{}/api/fs/list", self.base_url))
            .header("Authorization", &token)
            .json(&json!({"path": path, "page": 1, "per_page": 1, "refresh": true}))
            .send()
            .await
            .map_err(|e| format!("refresh request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("refresh parse: {e}"))?;
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| format!("refresh {path} failed"));
            return Err(m);
        }
        log::log(&format!("refresh dir OK (增量索引已触发): {path}"));
        Ok(())
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
                log::log(&format!("MKCOL {cur} FAILED -> {st} ({url}, user {})", self.username));
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
        log::log(&format!("PUT {rel}: begin ({} bytes)", total));
        let t0 = Instant::now();
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
            log::log(&format!("PUT {rel} FAILED -> {st} ({url}) {}", t.chars().take(120).collect::<String>()));
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
            T[(n >> 18) as usize & 63] as char,
            T[(n >> 12) as usize & 63] as char,
            if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' },
            if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
