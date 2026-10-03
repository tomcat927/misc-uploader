# AGENTS.md

本仓库是 agent 协作开发项目。任何 agent 会话进入本仓库工作前,先读完本文件。

## 项目概述

misc-uploader:Windows 桌面工具(拖拽上传文件/文件夹到用户的 OpenList 仓库,纯 REST API),SHA-256 内容去重,按日期自动归类,支持热更新。服务于用户的杂物数字仓库(tianyi-misc-repo,私有仓,不在这里)。

- 技术栈:**Tauri 2**(Rust 后端 `src-tauri/` + 零构建 HTML 前端 `ui/`,`withGlobalTauri`,无 npm 前端工具链)
- 设计文档:`DESIGN.md`(全部拍板决策与边界,改动方案前先读)
- 配套私有仓:`tomcat927/tianyi-misc-repo`(运维与凭据,本仓库永不包含其内容)

## 环境规约(重要)

- **本机零环境、零构建**。所有构建/打包在 GitHub Actions(windows-latest)。不要在本机安装 node_modules、cargo、任何工具链。
- 纯逻辑验证可用系统已有 node(零安装)直跑,但不要为此新增依赖。
- 迭代方式 = 修改代码 → push → CI → 看结果 → 修。一轮 CI 约 5-10 分钟(rust-cache 命中后更快)。

## CI/CD 规约

- workflow:`.github/workflows/app-build.yml`;触发:push 到 master / `v*` tag / 手动 dispatch
- **新 push 自动取消同分支进行中的旧 run**(concurrency + cancel-in-progress,照抄 openlist-uploader);监听时只看最新 run,它包含之前所有提交
- **每次 push 后必须监听构建结果直到 completed**(gh run watch 或 API 轮询);failure 则拉 `--log-failed` 分析修复重推,重复直到 success。这是硬性流程,不要构建一半就汇报完成。
- 产物:nsis 安装包 + .sig 上传 artifact;同时发布到固定 tag `latest` 的 Release(exe/sig/latest.json,--clobber)——该 Release 是热更新常驻通道,不能删。
- `v*` tag 触发正式 Release。

### 版本号

CI 按**中国时区**自动生成 `{年}.{月*100+日}.{时*100+分}`(如 `2026.1003.826`),replace 注入 Cargo.toml 与 tauri.conf.json。仓库内的 0.1.0 只是占位,不要手动改版本号。

### 已踩过的坑(全部已修,别再踩)

1. `npx tauri` 前必须 `npm ci`(否则 CLI 不存在)
2. 改 package.json 依赖后必须重新生成 package-lock.json(本机用 `npm install --package-lock-only`,零安装)
3. Rust 2024:if-let/元组表达式里的链式临时借用(`state.queue.lock().unwrap()` 等)会 E0597,拆成局部变量
4. **GITHUB_TOKEN 默认只读**:发 Release 的 job 必须显式 `permissions: contents: write`(403 Resource not accessible by integration 的根因)
5. Actions 的 pwsh 把 stderr 转 error record:gh 命令判断要用 `gh api ... | Out-String -match` 模式,或 `$ErrorActionPreference = 'Continue'`
6. `src-tauri/capabilities/default.json` 必须存在(`core:default`)——缺失时 getVersion/event listen 等核心 API 被静默拒绝(自定义 command 不受影响,所以表面正常,边缘功能先坏)
7. tokio::fs::File 的 `.read()` 需要 `use tokio::io::AsyncReadExt;`
8. **401 已是单一链路**(2026-10-04 迁移纯 REST 后):token 过期自动重登一次并重试(openlist.rs `call_with_relogin`),仍 401 = 凭据失效。历史坑:双协议时代 REST(token)/WebDAV(Basic)两套认证并存,「REST 登录成功但 WebDAV 401」需先分诊链路——已随 WebDAV 移除失效
9. **Tauri 返回值字段是 snake_case**:invoke 入参 JS camelCase→Rust snake_case 自动映射,但**返回值**按 serde 原样序列化(如 `has_password`)——前端写成 `s.hasPassword` 永远 undefined 且不报错(曾导致开机自动连接从未执行)
10. **`write!` 的 `{}` 对 u8 输出十进制数字不是字符**(曾把手写 base64 的前两位打成数字,所有 WebDAV 请求 401 而 REST 正常)——字节→字符必须 `as char`(手写 base64 已随 WebDAV 移除删除,教训本身通用)
11. **MutexGuard 非 Send,不得跨 `.await`**——即使显式 `drop(g)` 也可能被生成器分析判为非 Send(spawn 的 Future 必须 Send);把 await 移到守卫作用域之外,守卫用块级作用域包裹
12. **块尾表达式里的链式锁借用会 E0597**(`let x = { let st = app.state(); st.config.lock().unwrap().field };`)——锁的临时守卫存活期超过块内 `st` 的借用;把守卫绑定成具名局部变量再取字段

## 凭据与安全规约(不可妥协)

- **代码/配置里禁止出现任何服务器 IP、域名、账号、密码字面量**。仓库是公开的。
- CI secrets 清单:`TAURI_SIGNING_PRIVATE_KEY`/`_PASSWORD`(热更新签名)。CI 与任何真实服务器**零耦合**(2026-10-03 拍板移除冒烟测试:脚本独立于应用代码测不到 Rust 客户端,只会因服务器凭据变动挡构建;协议回归保护日后用离线 mock 测试覆盖)。
- **不要把用户凭据编译进安装包**(编译期 option_env! 注入 = 公开二进制可提取)。远程日志目标必须是用户在设置页配置的(当前实现,见 DESIGN.md 修订记录)。
- 私钥(`~/.tauri/misc-uploader.key`)只进 Secret;泄漏 = 热更新通道被伪造,需换钥重装。
- 涉及用户服务器/账号/权限的任何方案(例如自托管端点、改账号权限),**先陈述方案与风险,等用户拍板再动手**。

## 行为纪律

- **分层确认(2026-10-03 用户拍板,收窄原「技术结构逐层确认」范围)**:
  - **必须先拍板再动手**:涉及用户服务器/账号/权限的方案;可能丢失或破坏既有数据(去重历史、本机配置、远端文件)的改动;不可逆操作(删除、热更新通道/签名体系变更);技术选型/协议/存储结构等架构级决策。历史上两次因先斩后奏被叫停(其中一次擅自做自托管端点+改用户服务器被全部回滚),即此强制范围的由来。
  - **免确认直接做,做完汇报**:纯 UI 与展示层逻辑、缺陷修复、文档,以及其它**可逆且不碰服务器、不碰用户数据**的改动——push 本身即是确认。
  - 归类拿不准时用选项列表问,给推荐但让用户拍板。
- 每次实质变更后同步更新 `DESIGN.md`(拍板决策)与 README(功能/使用)。
