# misc-uploader

Windows 桌面工具:把文件/文件夹**拖拽上传**到你的 OpenList(WebDAV)仓库,带内容级 SHA-256 去重与按日期自动归类。Tauri 2 构建,安装包 ~8MB。

## 功能

- **拖拽即传**:拖文件或整个文件夹进窗口,自动递归展开入队
- **两种模式**:手动(左侧目录树选目标)/ 自动(按文件修改时间归 `auto/YYYY/MM/`)
- **内容去重**:上传前计算 SHA-256,同一内容无论拖到哪个目录只存一份(历史记录仅覆盖本软件上传过的文件)
- **可靠队列**:并发 3、失败自动重试 3 次(退避 2s/8s/30s)、实时状态与进度
- **权限隔离**:配合服务端创建 `base_path` 锁定的专用账号,软件能力边界 = 该目录内读写

## 安装

到 [Releases](../../releases) 下载 `misc-uploader_x.y.z_x64-setup.exe` 安装;或 Actions artifact 取开发版。

首次运行:打开「设置」→ 填 OpenList 地址(`https://host:5245`)、用户名、密码 → 保存并连接。

## 构建

本机零环境,构建全部在 GitHub Actions:push 即出 artifact,打 `v*` tag 发 Release。

## 服务端建议(OpenList)

建议为软件创建专用账号(而非 admin):

1. 管理后台新建用户,`base_path` 设为仓库根目录(如 `/misc`)——该用户所有操作被限制在此目录内
2. 客户端上传走 WebDAV(`/dav`),需要该账号有 WebDAV 权限

## 许可

MIT
