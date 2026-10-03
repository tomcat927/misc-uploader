// openlist.rs — OpenList client: 纯 REST(login/list/mkdir/put),rustls HTTPS。
// 2026-10-04 按私有仓 tianyi-misc-repo `docs/client-protocol-decision.md` 拍板移除 WebDAV 链路
// (MKCOL/PUT + Basic auth):建目录改 POST /api/fs/mkdir,上传改 PUT /api/fs/put(服务端流式)。
use crate::log;
use reqwest::{Client, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::RwLock;
use std::time::Instant;
use tokio_util::io::ReaderStream;

// REST 调用失败的两类:会话失效(401,可重登一次重试)与其它(终局失败)
enum ApiFail {
    Auth,
    Other(String),
}

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

// File-Path 头统一封装:整条路径百分号编码(含 `/`),服务端 url.PathUnescape 还原后
// 经 user.JoinPath 拼接用户 base_path(即客户端传 base 相对路径,与 list/mkdir 同语义)。
// 该头禁止在别处裸拼(拍板防御清单 3)。
fn file_path_header(p: &str) -> String {
    urlencoding::encode(p).into_owned()
}

// mkdir「已存在」容错:alist 系对已存在目录报 code 500,消息措辞随版本/驱动略有差异,匹配 exist 词根
fn is_exists_error(msg: &str) -> bool {
    msg.to_lowercase().contains("exist")
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

    // ---- REST 应答统一防线(拍板防御清单)----
    // 1) HTTP 401 / JSON code 401 → 会话失效,由 call_with_relogin 重登一次后重试(防循环);
    // 2) 强制 JSON 解析:非 JSON(打错路由打到 SPA 兜底页、反代错误页等 "200+HTML" 假成功)一律按失败处理。
    async fn api<T: DeserializeOwned>(&self, resp: Response) -> Result<ApiResp<T>, ApiFail> {
        let status = resp.status().as_u16();
        if status == 401 {
            return Err(ApiFail::Auth);
        }
        let text = resp
            .text()
            .await
            .map_err(|e| ApiFail::Other(format!("read body: {e}")))?;
        let parsed: ApiResp<T> = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                let snippet: String = text.chars().take(200).collect();
                log::log(&format!("non-JSON response (http {status}): {snippet}"));
                return Err(ApiFail::Other(format!("non-JSON response (http {status}): {e}")));
            }
        };
        if parsed.code == 401 {
            return Err(ApiFail::Auth);
        }
        Ok(parsed)
    }

    // 会话失效统一处理:401 → 重登一次并重试(防循环),其余错误原样上抛。
    // 调用方传入「以 token 为参执行一次请求」的闭包,重试时用新 token 重发(put 会重开文件流)。
    async fn call_with_relogin<T, F, Fut>(&self, label: &str, op: F) -> Result<ApiResp<T>, String>
    where
        T: DeserializeOwned,
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<ApiResp<T>, ApiFail>>,
    {
        let token = self
            .token
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| "not logged in".to_string())?;
        let resp = match op(token).await {
            Err(ApiFail::Auth) => {
                log::log(&format!("{label}: token expired, re-login once"));
                self.login().await?;
                let token = self
                    .token
                    .read()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| "not logged in".to_string())?;
                op(token).await
            }
            r => r,
        };
        match resp {
            Ok(r) => Ok(r),
            Err(ApiFail::Auth) => Err(format!("{label}: 401 after re-login")),
            Err(ApiFail::Other(e)) => Err(format!("{label}: {e}")),
        }
    }

    pub async fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        if self.token.read().unwrap().is_none() {
            self.login().await?;
        }
        let path_s = path.to_string();
        let resp = self
            .call_with_relogin(&format!("list {path}"), |token| {
                self.list_req(&path_s, token, false, 1000)
            })
            .await?;
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| format!("list {path} failed"));
            log::log(&format!("list {path} FAILED: {m}"));
            return Err(m);
        }
        let entries = resp.data.and_then(|d| d.content).unwrap_or_default();
        log::log(&format!("list {path}: {} entries", entries.len()));
        Ok(entries
            .into_iter()
            .map(|e| Entry { name: e.name, is_dir: e.is_dir, size: e.size })
            .collect())
    }

    // 上传成功后调用:refresh=true 穿透缓存,触发 OpenList 增量索引(尽力而为,失败不影响上传结果)
    pub async fn refresh_dir(&self, path: &str) -> Result<(), String> {
        if self.token.read().unwrap().is_none() {
            self.login().await?;
        }
        let path_s = path.to_string();
        let resp = self
            .call_with_relogin(&format!("refresh {path}"), |token| {
                self.list_req(&path_s, token, true, 1)
            })
            .await?;
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| format!("refresh {path} failed"));
            return Err(m);
        }
        log::log(&format!("refresh dir OK (增量索引已触发): {path}"));
        Ok(())
    }

    async fn list_req(
        &self,
        path: &str,
        token: String,
        refresh: bool,
        per_page: usize,
    ) -> Result<ApiResp<ListData>, ApiFail> {
        let url = format!("{}/api/fs/list", self.base_url);
        log::log(&format!("list request: POST {url} path={path} refresh={refresh}"));
        let resp = self
            .http
            .post(&url)
            .header("Authorization", &token)
            .json(&json!({"path": path, "page": 1, "per_page": per_page, "refresh": refresh}))
            .send()
            .await
            .map_err(|e| ApiFail::Other(format!("list request: {e}")))?;
        self.api(resp).await
    }

    // 逐级建目录(REST mkdir 不递归建父级)。"已存在"不算失败:错误消息含 exist 直接认;
    // 措辞不符时退一步 fs/list 确认该级目录确实在(防消息措辞变化导致每次上传都失败)。
    pub async fn mkdirp(&self, rel_dir: &str) -> Result<(), String> {
        if self.token.read().unwrap().is_none() {
            self.login().await?;
        }
        let mut cur = String::new();
        for seg in rel_dir.split('/').filter(|s| !s.is_empty()) {
            cur = if cur.is_empty() { seg.to_string() } else { format!("{cur}/{seg}") };
            let path_s = cur.clone();
            let resp = self
                .call_with_relogin(&format!("mkdir {cur}"), |token| self.mkdir_req(&path_s, token))
                .await?;
            if resp.code == 200 {
                continue;
            }
            let m = resp.message.unwrap_or_else(|| format!("code {}", resp.code));
            if is_exists_error(&m) {
                log::log(&format!("mkdir {cur}: already exists (code {}, {m})", resp.code));
                continue;
            }
            if self.dir_exists(&cur).await {
                log::log(&format!("mkdir {cur}: exists per list fallback (code {}, {m})", resp.code));
                continue;
            }
            log::log(&format!("mkdir {cur} FAILED: code {} {m}", resp.code));
            return Err(format!("mkdir {cur}: {m}"));
        }
        Ok(())
    }

    async fn mkdir_req(&self, path: &str, token: String) -> Result<ApiResp<serde_json::Value>, ApiFail> {
        let url = format!("{}/api/fs/mkdir", self.base_url);
        log::log(&format!("mkdir request: POST {url} path={path}"));
        let resp = self
            .http
            .post(&url)
            .header("Authorization", &token)
            .json(&json!({"path": path}))
            .send()
            .await
            .map_err(|e| ApiFail::Other(format!("mkdir request: {e}")))?;
        self.api(resp).await
    }

    // fs/list 确认目录是否已存在(mkdir 错误消息不可靠时的兜底)
    async fn dir_exists(&self, path: &str) -> bool {
        let p = std::path::Path::new(path);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            return false;
        }
        let parent = p
            .parent()
            .map(|x| x.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        matches!(
            self.list(&parent).await,
            Ok(entries) if entries.iter().any(|e| e.is_dir && e.name == name)
        )
    }

    // 上传:PUT /api/fs/put,请求体 = 文件流(256KB 缓冲,内存恒定)。服务端 FsStream 把 body
    // 作为流直接交给存储层(server/handles/fsup.go 源码核验),客户端与服务端都不整读。
    // 头:File-Path(file_path_header 统一编码)/ Content-Length(已知大小)/ X-File-Sha256
    // (声明元数据,与去重同一次哈希;服务端记入 HashInfo,不做强校验)/ Overwrite 缺省 = 覆盖
    // (与原 WebDAV PUT 语义一致)。
    pub async fn put_file(&self, rel: &str, file_path: &str, sha256: Option<&str>) -> Result<u64, String> {
        if self.token.read().unwrap().is_none() {
            self.login().await?;
        }
        let total = tokio::fs::metadata(file_path)
            .await
            .map_err(|e| format!("stat {file_path}: {e}"))?
            .len();
        log::log(&format!("PUT {rel}: begin ({} bytes)", total));
        let t0 = Instant::now();
        let rel_s = rel.to_string();
        let fp_s = file_path.to_string();
        let sha_s = sha256.map(|s| s.to_string());
        let resp = self
            .call_with_relogin(&format!("PUT {rel}"), |token| {
                self.put_req(&rel_s, &fp_s, sha_s.as_deref(), token, total)
            })
            .await?;
        if resp.code != 200 {
            let m = resp.message.unwrap_or_else(|| "upload rejected".into());
            log::log(&format!("PUT {rel} FAILED: code {} {m}", resp.code));
            return Err(format!("PUT {rel} -> {m}"));
        }
        log::log(&format!("PUT {rel}: complete ({} bytes, {:.1}s)", total, t0.elapsed().as_secs_f64()));
        Ok(total)
    }

    async fn put_req(
        &self,
        rel: &str,
        file_path: &str,
        sha256: Option<&str>,
        token: String,
        total: u64,
    ) -> Result<ApiResp<serde_json::Value>, ApiFail> {
        let file = tokio::fs::File::open(file_path)
            .await
            .map_err(|e| ApiFail::Other(format!("open {file_path}: {e}")))?;
        let stream = ReaderStream::with_capacity(file, 256 * 1024);
        let url = format!("{}/api/fs/put", self.base_url);
        let mut req = self
            .http
            .put(&url)
            .header("Authorization", &token)
            .header("File-Path", file_path_header(rel))
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", total.to_string());
        if let Some(s) = sha256 {
            req = req.header("X-File-Sha256", s);
        }
        let resp = req
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| ApiFail::Other(format!("PUT {rel}: {e}")))?;
        self.api(resp).await
    }
}
