# dsh-desktop-host

**DSH Desktop 窗口宿主**（Rust / Tauri v2）——为 [dsh-desktop-tools](https://github.com/YUEEEEY/dsh-desktop-tools) 插件提供桌面窗口。

宿主只做三件事：**打开 WebView 窗口渲染 dsh web 界面、启动/看护 dsh 服务、退出时清理**。
运行时管理、Windows 补丁、面板、计费等能力全部在 dsh 插件里——**换宿主不改变环境**。

## 平台支持

| 平台 | 架构 | 说明 |
|---|---|---|
| Windows | x64 | WebView2（Win10/11 自带） |
| macOS | arm64 / x64 | WKWebView（macOS 11+） |
| Linux | x64 | WebKitGTK（需系统依赖） |

## 构建

需要 Rust 工具链（https://rustup.rs）。

```bash
# 克隆源码
git clone https://github.com/YUEEEEY/dsh-desktop-host.git
cd dsh-desktop-host

# 构建 release 二进制
cargo build --release
```

产物：`target/release/dsh-desktop`（Windows 为 `dsh-desktop.exe`）。

**Linux 额外依赖（Debian/Ubuntu）**：

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libgtk-3-dev patchelf
```

## 使用

构建后把二进制路径告诉插件（二选一）：

```bash
# 方式一：环境变量
export DSH_DESKTOP_BIN=/path/to/dsh-desktop-host/target/release/dsh-desktop

# 方式二：插件配置 desktopBin（在 profile 的 cordis.patch.yml 里）
# - id: desktop-tools
#   config:
#     desktopBin: /path/to/dsh-desktop-host/target/release/dsh-desktop
```

然后 `dsh web` 会自动打开桌面窗口；`dsh web --no-desktop` 可关闭自动开窗。

宿主命令行参数：

```
dsh-desktop [options]
  --url <url>       服务已由外部启动，直接打开该地址的窗口
  --serve           宿主自己启动（或连接）dsh web 服务（默认）
  --port <n>        （--serve 时）服务端口
  --workspace <dir> （--serve 时）工作区
  --home <dir>      DSH_HOME
```

窗口内快捷键：`Ctrl+Shift+P` 打开环境面板，`Ctrl+Shift+H` 回到主界面（或菜单"视图"）。

## 开发

```
host/
├─ src/
│  ├─ lib.rs        # Tauri 应用：窗口、视图切换、菜单、启动参数
│  ├─ server.rs     # 启动/看护 dsh 服务进程
│  ├─ runtime.rs    # 定位 dsh 安装（DSH_RUNTIME_DIR / npm 全局）
│  └─ settings.rs   # 端口/工作区设置
├─ tauri.conf.json
└─ Cargo.toml
```

## License

MIT
