# misc-uploader 设计文档(2026-10-03 全部拍板,终版)

## 需求

整理磁盘时:打开软件 → 选远端目录(或自动按日期)→ 拖入文件(含文件夹)→ 后台可靠上传到 misc 仓库。

## 拍板决策一览

| 决策点 | 结论 |
|---|---|
| 应用框架 | **Tauri 2**(Rust 后端 + 零构建 HTML 前端);包 ~8MB/内存 ~40MB;全场景对比后终选 |
| 构建/发布 | **GitHub Actions windows-latest;本机零环境零构建**。双轨:push → artifact 自测;`v*` tag → 正式 Release |
| 去重 | **内容级 SHA-256**:同 hash 无论拖到哪个目录都跳过(提示云上已有路径);历史仅覆盖本软件上传的文件 |
| 历史存储 | **JSON 文件**(零原生依赖,上限 1 万条自动截断) |
| 上传协议 | **WebDAV**(PUT/MKCOL + Basic auth),标准协议不绑私有接口;元数据(登录/列目录)走 OpenList REST API |
| 自动归类 | 按文件修改时间 → `auto/YYYY/MM/` |
| 账号 | 专用 `misc-uploader`(role=0,base_path=/misc chroot,permission=255);软件永不持有 admin 密码 |
| 前端构建 | **零构建**:纯 HTML/CSS/JS,`withGlobalTauri` 用 window.__TAURI__,无 npm 前端工具链 |
| 设置页交互 | **无保存按钮,字段失焦(change)即自动落盘**;「连接」按钮 = 保存+连接一步(连接本身就是最好的测试,独立 test_connection 命令已删);远程日志仅留「测试日志通道」「打开本地日志文件夹」两个动作按钮 |
| 设置页回显 | **保存即落盘(含连接失败)**:用户填的服务器/账号/密码即使登录失败也保留(内存+配置文件),重进设置页原样回显,连接失败只影响「已连接」状态。**密码框掩码回显**:已存密码显示 8 位掩码点(真实密码不下发前端),聚焦全选、输入即替换、清空后保存 = 沿用;后端以 `"********"` 为哨兵值 = 沿用已存密码。**启动即回显**:开机落到设置页必须走 openSettings 回填(直接 showView 会留空表单,保存一次就把已存 url/用户名覆盖成空——2026-10-03 热更新后"配置丢失"事故根因);空服务器/用户名的保存直接拒绝且不落盘 |
| 页面文字 | 内容文字可选中复制;仅按钮/目录树节点禁选(防误选);拖拽高亮只对文件拖拽生效 |
| 远程日志验证 | 设置页独立「测试日志通道」按钮(REST 登录 + MKCOL 目标目录逐级验证可写,幂等);**保存本身不校验**——自动保存不做网络请求,与「保存不丢用户输入」一致;后台每 5 分钟同步失败仅记本地日志 |
| 远程日志协议与路径 | **协议保持 WebDAV**(2026-10-03 拍板,不抄 openlist-uploader 的 REST);**存放路径是设置页字段** `remote_dir`(默认 `本地磁盘/misc-uploader/logs`,相对 logger 账号 base_path,用户可改)——对应规格 `/本地磁盘/{appName}/logs`;参考仓库 openlist-uploader 的 target_path 模式 |
| CI 服务器依赖 | **零**:CI 不登录任何真实服务器(2026-10-03 移除冒烟测试);应用的服务器/账号全部用户运行时配置,凭据不进 CI secrets |

## 边界(V1 不做)

剪贴板监控 / 全局热键 / 右键菜单 / 托盘 / 下载预览 / 删除远端 / 加密直传 vault(维持热层+cron 归档架构)/ 断点续传(整文件重试)/ 多服务器 / 自动更新 / 代码签名 / 文件内字节级进度(文件粒度状态,V2 再说)。

## 架构

```
ui/(纯 HTML/CSS/JS):目录树 | 拖拽区(视觉反馈)| 队列列表
   ↕ invoke() / listen()  (withGlobalTauri,无 npm 前端构建)
src-tauri(Rust):
   openlist.rs  reqwest:REST 登录/列目录 + WebDAV PUT/MKCOL(rustls)
   queue.rs     tokio 并发3 / 重试3次退避(2s/8s/30s)/ sha2 流式 / history.json
   lib.rs       commands(connect/list_dir/retry/clear)+ DragDrop 事件(窗口级,原生路径!)
   ↕ HTTPS(rustls)
OpenList(WebDAV + REST)→ /opt/misc 热层 → cron 加密归档天翼(既有系统,软件零感知)
```

Tauri 优势落地点:**拖拽由 Rust 原生事件接收**(on_window_event DragDrop),文件/文件夹直接给绝对路径,前端无需 webkitGetAsEntry 递归遍历。

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
