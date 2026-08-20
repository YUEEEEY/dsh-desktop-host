# dsh-desktop-host

**DSH Desktop 窗口宿主**（Rust / Tauri v2）——为 [dsh-desktop-tools](https://github.com/YUEEEEY/dsh-desktop-tools) 插件提供桌面窗口。

[![License: MIT](https://img.shields.io/badge/License-MIT-2EA44F.svg)](LICENSE)
[![Windows x64](https://img.shields.io/badge/Windows-x64-4D6BFE.svg)](https://github.com/YUEEEEY/dsh-desktop-host/releases)
[![macOS](https://img.shields.io/badge/macOS-arm64%20%7C%20x64-4D6BFE.svg)](https://github.com/YUEEEEY/dsh-desktop-host/releases)
[![Linux](https://img.shields.io/badge/Linux-x64-4D6BFE.svg)](https://github.com/YUEEEEY/dsh-desktop-host/releases)

宿主以 WebView 封装 dsh web 界面，负责**启动/看护服务进程、窗口与托盘、环境面板弹窗、代码编辑器入口**。
运行时管理、平台兼容、计费等能力全部在 dsh 插件里——**换宿主不改变环境**。

## 平台支持

| 平台 | 架构 | 说明 |
|---|---|---|
| Windows | x64 | WebView2（Win10/11 自带） |
| macOS | arm64 / x64 | WKWebView（macOS 11+） |
| Linux | x64 | WebKitGTK（需系统依赖） |

## 安装

安装 [dsh-desktop-tools](https://github.com/YUEEEEY/dsh-desktop-tools) 插件时，
宿主二进制会按当前操作系统**自动从 GitHub Release 下载**（找不到对应资产时才回退到源码构建），
无需手动编译。也可从 [Releases](https://github.com/YUEEEEY/dsh-desktop-host/releases) 直接下载对应平台的二进制，
然后设置 `DSH_DESKTOP_BIN` 环境变量或插件配置 `desktopBin` 指向它。

### 手动构建

需要 Rust 工具链（https://rustup.rs）。

```bash
git clone https://github.com/YUEEEEY/dsh-desktop-host.git
cd dsh-desktop-host
cargo build --release
# 产物：target/release/dsh-desktop（Windows 为 dsh-desktop.exe）
```

**Linux 额外依赖（Debian/Ubuntu）**：

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libgtk-3-dev patchelf
```

## 功能

- **系统托盘**：显示/隐藏窗口、打开环境面板（弹窗）、打开代码编辑器、在浏览器打开、重启服务、检查更新、开机自启、退出；tooltip 显示 dsh 运行状态。
- **环境面板弹窗**：`Ctrl+Shift+P` 或托盘/菜单打开独立面板窗口（运行时版本 / 更新 / 平台兼容 / 计费摘要）。
- **代码编辑器**：`Ctrl+Shift+E` 或托盘/菜单在主窗口内打开代码编辑器（浏览与编辑工作区文件）。
- **自动更新提示**：每 6 小时检查一次 dsh 运行时更新，发现新版本时在托盘提示（也可手动"检查更新"）。
- **单实例**：重复启动时聚焦已有窗口，不会产生多个服务进程。
- **开机自启**：托盘一键开关，状态持久化。
- **窗口状态记忆**：记住上次的窗口位置与尺寸。
- **服务看护**：dsh 服务异常退出时自动重启（最多 3 次，退避重试）。

## 使用

```bash
# 方式一：交给插件（推荐）
dsh plugin --profile web add dsh-desktop-tools
dsh web   # 服务就绪后自动打开桌面窗口

# 方式二：手动指定宿主
export DSH_DESKTOP_BIN=/path/to/dsh-desktop
dsh web
```

`dsh web --no-desktop` 可关闭自动开窗。

宿主命令行参数：

```
dsh-desktop [options]
  --url <url>       服务已由外部启动，直接打开该地址的窗口
  --serve           宿主自己启动（或连接）dsh web 服务（默认）
  --port <n>        （--serve 时）服务端口
  --workspace <dir> （--serve 时）工作区
  --home <dir>      DSH_HOME
```

窗口内快捷键：`Ctrl+Shift+P` 环境面板（弹窗）、`Ctrl+Shift+E` 代码编辑器、`Ctrl+Shift+H` 回到主界面（或菜单"视图"）。

## 开发

```
host/
├─ src/
│  ├─ lib.rs        # Tauri 应用：窗口、托盘、菜单、面板弹窗、更新检查、启动参数
│  ├─ server.rs     # 启动/看护 dsh 服务进程
│  ├─ runtime.rs    # 定位 dsh 安装（DSH_RUNTIME_DIR / npm 全局）
│  └─ settings.rs   # 端口/工作区/自启/窗口状态设置
├─ tauri.conf.json
└─ Cargo.toml
```

发布新版本：打 tag（如 `v0.1.0`）→ GitHub Actions 自动构建四平台二进制并创建 Release。

## License

MIT
