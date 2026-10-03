# misc-uploader 设计文档(2026-10-03 拍板;2026-10-04 协议迁移修订)

## 需求

整理磁盘时:打开软件 → 选远端目录(或自动按日期)→ 拖入文件(含文件夹)→ 后台可靠上传到 misc 仓库。

## 拍板决策一览

| 决策点 | 结论 |
|---|---|
| 应用框架 | **Tauri 2**(Rust 后端 + 零构建 HTML 前端);包 ~8MB/内存 ~40MB;全场景对比后终选 |
| 构建/发布 | **GitHub Actions windows-latest;本机零环境零构建**。双轨:push → artifact 自测;`v*` tag → 正式 Release |
| 去重 | **内容级 SHA-256**:同 hash 无论拖到哪个目录都跳过(提示云上已有路径);历史仅覆盖本软件上传的文件 |
| 历史存储 | **JSON 文件**(零原生依赖,上限 1 万条自动截断) |
| 上传协议 | **纯 REST token**(2026-10-04 迁移,拍板记录见私有仓 tianyi-misc-repo `docs/client-protocol-decision.md`):登录/列目录/建目录/上传全走 OpenList API;上传 = `PUT /api/fs/put` 流式,内存恒定 |
| 自动归类 | 按文件修改时间 → `auto/YYYY/MM/` |
| 账号 | 专用 `misc-uploader`(role=0,base_path=/misc chroot,permission=255);软件永不持有 admin 密码 |
| 前端构建 | **零构建**:纯 HTML/CSS/JS,`withGlobalTauri` 用 window.__TAURI__,无 npm 前端工具链 |
| 设置页交互 | **无保存按钮,字段失焦(change)即自动落盘**;「连接」按钮 = 保存+连接一步(连接本身就是最好的测试,独立 test_connection 命令已删);远程日志动作按钮 = 「测试日志通道」「**立即上传**」(2026-10-04 增,手动推一次,不经定时器、不看 enabled 开关)「打开本地日志文件夹」 |
| 设置页回显 | **保存即落盘(含连接失败)**:用户填的服务器/账号/密码即使登录失败也保留(内存+配置文件),重进设置页原样回显,连接失败只影响「已连接」状态。**密码框掩码回显**:已存密码显示 8 位掩码点,聚焦全选、输入即替换、清空后保存 = 沿用;后端以 `"********"` 为哨兵值 = 沿用已存密码。**「显示」按钮可查看真实密码**(2026-10-03 修订,用户要求):点「显示」时前端经专用 `reveal_password` 命令按需取回真实密码(常规回显仍是掩码;密码本就明文存本机 config,UI 按需展示不增加暴露面),收起恢复掩码。**启动即回显**:开机落到设置页必须走 openSettings 回填(直接 showView 会留空表单,保存一次就把已存 url/用户名覆盖成空——2026-10-03 热更新后"配置丢失"事故根因);空服务器/用户名的保存直接拒绝且不落盘 |
| 队列与目录树显示(2026-10-03 批) | **队列每行两行式**:上行 = 文件名/大小/进度条/状态;下行 = 本地完整路径 → `/misc/` 远程路径(rel 在进入处理时即显示,不必等完成)。**远端目录树展开时同时列出文件**(仅展示、不可选为目标,带大小;数据在展开时实时拉取),标题栏加「⟳ 刷新」按钮,刷新/新建文件夹后**保留当前选中目标**(沿路径逐级重展开并高亮)。**拖入文件夹保留内部目录结构**:目标 rel = `目标/拖入根文件夹名/子路径/文件名`(自动模式 = `auto/年/月/拖入根/…`),消除旧版拍平导致不同子目录同名文件互相覆盖的数据丢失隐患;单文件拖入行为不变 |
| 队列排序与上传历史(2026-10-03 批) | 队列**分组排序(拍板方案 A)**:进行中(hashing/pending/processing/uploading/cooldown)按入队序置顶稳定;已完成(done/skipped/failed)按完成时间排,默认最新在前(QueueItem 新增 `finished_at` 打点,重试重新入队时清掉)。队列栏「↓ 新在前/↑ 旧在前」按钮只翻转已完成区块方向,记忆存 localStorage(零构建、热更新不丢);排序纯展示层,不影响先入先传的上传顺序与 retry/clear。**上传历史面板**:主视图队列栏「📜 历史」展开/收起,跨会话读 history.json(内容级去重,每条 = 远程路径 + 完成时间,同一内容记最新位置,上限 1 万条);`get_history` 命令按完成时间倒序分页(200/页,加载更多/⟳ 刷新),已连接读内存(put 即落盘,与磁盘等价)、未连接直接读盘 |
| 页面文字 | 内容文字可选中复制;仅按钮/目录树节点禁选(防误选);拖拽高亮只对文件拖拽生效 |
| 远程日志验证 | 设置页独立「测试日志通道」按钮(REST 登录 + fs/mkdir 目标目录逐级验证可写,幂等);**保存本身不校验**——自动保存不做网络请求,与「保存不丢用户输入」一致;后台每 5 分钟同步失败仅记本地日志 |
| 远程日志协议与路径 | **协议随主链路迁移为 REST**(2026-10-04;原「保持 WebDAV」拍板被私有仓 client-protocol-decision.md 取代);**存放路径是设置页字段** `remote_dir`(默认 `misc-uploader/logs`,**相对 logger 账号可见根**,服务端拼 base_path——logger 账号实际 base_path=/本地磁盘,旧默认 `本地磁盘/misc-uploader/logs` 曾双写为 /本地磁盘/本地磁盘/...,WebDAV 时代即如此,2026-10-04 修正)——对应规格 `/本地磁盘/{appName}/logs`;**上传间隔** `sync_interval_minutes` 也是设置页字段(默认 5 分钟);参考仓库 openlist-uploader 的 target_path 模式 |
| 用户偏好(2026-10-03 批量) | 设置页**加宽加大字号**(1080px/14px 基准);密码框显示/隐藏切换;上传并发数(1-16,默认 3)与最大重试次数(0-10,默认 3)可设置,并发数下次「连接」生效;上传成功后 `list refresh=true` 刷新目标目录触发 OpenList 增量索引(尽力而为);开机自启(tauri-plugin-autostart,注册参数 `--autostart`)+ 静默启动(配置勾选或 `--autostart` 启动即隐藏窗口)+ 托盘(左键/菜单唤出,菜单含退出)+ 关闭最小化到托盘(`minimize_on_close` 默认开)均可配置;启动自动检查更新(可关) |
| CI 服务器依赖 | **零**:CI 不登录任何真实服务器(2026-10-03 移除冒烟测试);应用的服务器/账号全部用户运行时配置,凭据不进 CI secrets |

## 边界(V1 不做)

剪贴板监控 / 全局热键 / 右键菜单 / 托盘 / 下载预览 / 删除远端 / 加密直传 vault(维持热层+cron 归档架构)/ 断点续传(整文件重试)/ 多服务器 / 自动更新 / 代码签名 / 文件内字节级进度(文件粒度状态,V2 再说)。

## 架构

```
ui/(纯 HTML/CSS/JS):目录树 | 拖拽区(视觉反馈)| 队列列表
   ↕ invoke() / listen()  (withGlobalTauri,无 npm 前端构建)
src-tauri(Rust):
   openlist.rs  reqwest:纯 REST 登录/列目录/建目录/上传(流式 body,rustls)
   queue.rs     tokio 并发3 / 重试3次退避(2s/8s/30s)/ sha2 流式 / history.json
   lib.rs       commands(connect/list_dir/retry/clear)+ DragDrop 事件(窗口级,原生路径!)
   ↕ HTTPS(rustls)
OpenList(REST API)→ /opt/misc 热层 → cron 加密归档天翼(既有系统,软件零感知)
```

Tauri 优势落地点:**拖拽由 Rust 原生事件接收**(on_window_event DragDrop),文件/文件夹直接给绝对路径,前端无需 webkitGetAsEntry 递归遍历。

## 纯 REST 设计(2026-10-04 迁移,取代原「REST 元数据 + WebDAV 数据」双协议)

原双协议(元数据 REST + 数据 WebDAV)的三条理由经逐条核验后不成立/权重极低,已按私有仓
tianyi-misc-repo `docs/client-protocol-decision.md`(2026-10-03 拍板,含服务端源码核验,即本架构
决策的拍板记录)整体迁移为纯 REST token:登录、列目录、建目录、上传全部走 OpenList API,单一认证链。

| 操作 | 端点 | 要点 |
|---|---|---|
| 登录(拿 token) | REST `POST /api/auth/login` | JSON body 用户名+密码 |
| 列目录(目录树) | REST `POST /api/fs/list` | `Authorization: <token>`;上传成功后 `refresh:true` 触发增量索引 |
| 建目录 | REST `POST /api/fs/mkdir` | 逐级创建(mkdir 不递归建父级);「已存在」按成功处理(消息含 exist 直接认,措辞不符时 fs/list 兜底确认) |
| 上传 | REST `PUT /api/fs/put` | 请求体 = 文件流(256KB 缓冲,内存恒定);`File-Path`/`Content-Length`/`X-File-Sha256` 头,`Overwrite` 缺省 = 覆盖 |

**路径语义**:所有 API 路径为 base 相对路径,服务端经 `user.JoinPath` 拼接用户 base_path
(fsup.go/fsmanage.go 源码核验),与列目录同语义、与原 WebDAV 行为一致,迁移零配置变化。

**防御清单(全部实现于 openlist.rs,拍板文档 §3)**:
1. 所有响应强制 JSON 解析 + `code==200` 校验——非 JSON(反代错误页、SPA 兜底页等"200+HTML"假成功)一律按失败处理;
2. 401(HTTP 状态或 JSON code)→ 重登一次并重试,防循环(`call_with_relogin`);
3. `File-Path` 编码统一封装(`file_path_header`),禁止裸拼。

**迁移动机(简,详证见拍板文档 §2)**:① 原理由「REST 整文件读内存」不成立——openlist-uploader 的
`fs::read` 是其客户端实现选择;OpenList 服务端 FsStream 把 `c.Request.Body` 作为流直接交给存储层,
客户端 reqwest 流式 body 内存同样恒定;② 「标准协议可换服务器」期权价值极低——系统锁定在天翼驱动/
Crypt/misc-sync.py,metadata 链路早已 REST,且灾备恢复不依赖客户端协议;③ 单一认证链消灭
「REST 成功但 WebDAV 401」分诊与手写 base64(历史坑 8/10)。账号最小权限 = bit3(写/建目录);
原记录的 permission=255 与 WebDAV 权限位(bit8/9)的规格矛盾随迁移自然消除。

「已连接」徽章语义 = 内存里持有一次成功 `login()` 的会话(启动时配置齐全则自动连接);REST 上传
复用该会话 token,401 自动重登使其在长队列/挂机场景下自愈。

## 踩坑记录

### 日志存放目录双写 /本地磁盘/本地磁盘/...(2026-10-04)

**现象**:磁盘上出现名为 `本地磁盘` 的目录,日志实际落在 `/本地磁盘/本地磁盘/misc-uploader/logs`。
自 2026-10-03 11:20(WebDAV 时代首次日志同步)即如此;REST 迁移后行为完全一致——双侧核验确认
**不是迁移引入的**,是字段语义从第一天就理解错了。

**根因(三层)**:
1. 「日志存放目录」`remote_dir` 与所有 OpenList API 路径一样,是**相对账号可见根(base_path)**的
   路径——服务端 `JoinBasePath` = `stdpath.Join(base, path)`,**纯拼接、无前缀去重**
   (pkg/utils/path.go 源码核验);
2. logger 账号 base_path=`/本地磁盘`(chroot 专用账号),不是整个仓库根;
3. 字段默认值/私有仓文档当初**照抄了 openlist-uploader 的 target_path 绝对式填法**——但那个应用
   的账号 base=`/`(admin 类,可见全仓库),填 `/本地磁盘/xxx` 才正确;两个应用账号可见根不同,
   同一写法在 logger 账号下前缀被拼了两次。

**为什么长期未发现**:应用对不存在的目录逐级自动创建,双写静默成功不报错;按绝对路径也确实能
拉到日志(拉到的就是双层位置),私有仓文档写的路径从未真实成立却"看起来能用"。

**拍板(2026-10-04,用户采纳 A)**:logger 账号**保持 chroot**(permission=65535 全权限账号,
圈住可见范围是刻意的安全设计),字段与默认值改为相对式 `misc-uploader/logs`;**不采纳 B**
(把 logger base_path 放开为 `/` 以统一两应用的绝对式写法——base 一开该账号即可见天翼盘/vault
全仓库,安全面变大)。残留双层目录 `/本地磁盘/本地磁盘` 由用户手动清理(可选,不影响运行)。

**通用教训**:任何 OpenList API 路径/目录类设置字段,填的都是「相对该账号可见根」的路径;
跨应用抄配置前,先核对两边账号的 base_path 是否同一语义。

## CI(.github/workflows/app-build.yml)

push(app/** 触发):
1. windows-latest + dtolnay/rust-toolchain + rust-cache
2. PowerShell System.Drawing 生成源 icon → `npx @tauri-apps/cli icon`(产出 ico/icns/png 全套)
3. ~~质量门:PowerShell 冒烟~~ → **已移除(2026-10-03 拍板)**:脚本用 PowerShell 独立实现登录/列目录,测不到 Rust 客户端代码,只能证明服务器可用;CI 改为与任何真实服务器零耦合,应用连接全部由用户在设置页配置
4. `npx tauri build`(nsis bundle)
5. upload-artifact(misc-uploader-setup-<sha>.exe)

打 `v*` tag:同流程 + softprops/action-gh-release 发布正式 Release(双轨中的 Release 轨)。

## 里程碑

- M1 Rust 骨架:openlist.rs + CI 冒烟通过
- M2 队列 + commands + UI 拖拽闭环
- M3 首个 nsis artifact 下载验证 + tag Release
- V2 候选:文件内进度、DPAPI 加密配置、右键菜单、托盘

## 密码体系(重复以示重要)

软件配置 = misc-uploader 账号(明文存本机 app config,V2 DPAPI);与 Crypt password/salt 无关;密码不进 git(Secrets 与服务器 root-only 文件)。
