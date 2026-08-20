mod runtime;
mod server;
mod settings;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

pub struct AppState {
    pub settings: Mutex<settings::Settings>,
    pub server_child: Mutex<Option<std::process::Child>>,
    pub parsed_url: Mutex<Option<String>>,
    pub current_url: Mutex<Option<String>>,
    /// 当前主窗口视图："harness"（主界面）/ "editor"（代码编辑器）
    pub current_view: Mutex<String>,
    /// 主窗口缩放倍率（菜单"窗口"控制）
    pub zoom: Mutex<f64>,
    pub was_ready: AtomicBool,
    pub restart_tries: AtomicU32,
    pub log_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub settings_file: PathBuf,
}

const TRAY_ID: &str = "main-tray";

pub fn log(app: &AppHandle, line: &str) {
    // stderr 不可用时绝不能 panic：release 配置 panic=abort，一次日志写入失败会直接终止进程。
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{}", line);
    let st = app.state::<AppState>();
    let f = st.log_dir.join("desktop.log");
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
        use std::io::Write;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(fh, "[{}] {}", ts, line);
    }
}

pub fn find_url(line: &str) -> Option<String> {
    let idx = line.find("http://")?;
    let rest = &line[idx..];
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let url = &rest[..end];
    if url.contains("127.0.0.1") || url.contains("localhost") {
        Some(url.to_string())
    } else {
        None
    }
}

pub fn emit_ready(app: &AppHandle, url: &str) {
    let _ = app.emit("desktop:ready", format!("已连接 dsh Web 界面：{}", url));
}

/// 状态事件（宿主窗口加载的是远程 dsh web 页面，事件主要用于日志/调试）
pub fn emit(app: &AppHandle, phase: &str, message: &str, detail: &str) {
    let _ = app.emit(
        "desktop:status",
        format!("[{}] {} {}", phase, message, detail),
    );
}

/// 当前服务基址（http://host:port，不含路径）
fn base_url(app: &AppHandle) -> String {
    let st = app.state::<AppState>();
    let url = st
        .current_url
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:3080".to_string());
    match url.find('/') {
        Some(i) if i > 7 => url[..i].to_string(),
        _ => url.trim_end_matches('/').to_string(),
    }
}

/// 当前服务端口（从已解析的 URL 推导，取不到时退回配置端口）
fn service_port(app: &AppHandle) -> u16 {
    let st = app.state::<AppState>();
    let url = st.parsed_url.lock().unwrap().clone();
    let port = url
        .as_deref()
        .and_then(|u| u.strip_prefix("http://"))
        .and_then(|rest| rest.split('/').next())
        .and_then(|hp| hp.rsplit_once(':'))
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .or_else(|| {
            st.current_url
                .lock()
                .unwrap()
                .clone()
                .and_then(|u| u.strip_prefix("http://").map(|s| s.to_string()))
                .and_then(|rest| rest.split('/').next().map(|s| s.to_string()))
                .and_then(|hp| hp.rsplit_once(':').map(|(_, p)| p.to_string()))
                .and_then(|p| p.parse::<u16>().ok())
        })
        .unwrap_or_else(|| st.settings.lock().unwrap().port);
    port
}

/// 主窗口导航到指定路径（路径以 / 开头）
fn navigate_main(app: &AppHandle, path: &str) {
    let target = format!("{}{}", base_url(app), path);
    *app.state::<AppState>().current_view.lock().unwrap() = if path == "/editor" {
        "editor".to_string()
    } else {
        "harness".to_string()
    };
    log(app, &format!("视图切换：{} → {}", if path == "/editor" { "editor" } else { "harness" }, target));
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(u) = tauri::Url::parse(&target) {
            let _ = win.navigate(u);
        }
    }
}

/// 打开环境面板：独立弹窗（工具窗模型），已存在则聚焦复用。
fn open_panel(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("panel") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let url = format!("{}/panel", base_url(app));
    let Ok(parsed) = tauri::Url::parse(&url) else {
        log(app, "面板地址解析失败");
        return;
    };
    match tauri::WebviewWindowBuilder::new(app, "panel", tauri::WebviewUrl::External(parsed))
        .title("DSH 环境面板")
        .inner_size(920.0, 680.0)
        .min_inner_size(640.0, 480.0)
        .center()
        .on_navigation(|url| handle_webview_navigation(url))
        .build()
    {
        Ok(win) => {
            let _ = win.set_focus();
            log(app, "已打开环境面板弹窗");
        }
        Err(e) => log(app, &format!("打开环境面板失败：{}", e)),
    }
}

/// 视图切换：面板 → 弹窗；编辑器 / 主界面 → 主窗口导航。
/// 由原生菜单 / 快捷键（Ctrl+Shift+P / Ctrl+Shift+E / Ctrl+Shift+H）与托盘触发。
pub fn switch_view(app: &AppHandle, view: &str) {
    match view {
        "panel" => open_panel(app),
        "editor" => navigate_main(app, "/editor"),
        _ => navigate_main(app, "/"),
    }
}

/* ---------------- 托盘 ---------------- */

/// 系统托盘：显示/隐藏、面板弹窗、代码编辑器、浏览器打开、重启服务、检查更新、开机自启、退出。
fn build_tray(app: &AppHandle) -> Result<(), tauri::Error> {
    let toggle = MenuItem::with_id(app, "tray-toggle", "显示 / 隐藏窗口", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let panel = MenuItem::with_id(app, "tray-panel", "打开环境面板", true, None::<&str>)?;
    let editor = MenuItem::with_id(app, "tray-editor", "打开代码编辑器", true, None::<&str>)?;
    let browser = MenuItem::with_id(app, "tray-browser", "在浏览器打开", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "tray-restart", "重启 dsh 服务", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "tray-check", "检查更新", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(app, "tray-autostart", "开机自启", true, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&toggle, &sep, &panel, &editor, &sep, &browser, &restart, &check, &sep, &autostart, &sep, &quit],
    )?;

    // 图标：优先默认窗口图标，否则回退内嵌 32x32 png
    let icon = app
        .default_window_icon()
        .cloned()
        .or_else(|| tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")).ok());

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("DSH Desktop");
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }
    builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-toggle" => {
                if let Some(win) = app.get_webview_window("main") {
                    if win.is_visible().unwrap_or(false) {
                        let _ = win.hide();
                    } else {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            }
            "tray-panel" => open_panel(app),
            "tray-editor" => switch_view(app, "editor"),
            "tray-browser" => {
                let _ = opener::open(base_url(app));
            }
            "tray-restart" => {
                emit(app, "restart", "正在重启 dsh 服务…", "");
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    match server::restart(&handle).await {
                        Ok(()) => emit(&handle, "ready", "dsh 服务已重启", ""),
                        Err(e) => emit(&handle, "error", &format!("重启失败：{}", e), ""),
                    }
                });
            }
            "tray-check" => check_updates_now(app),
            "tray-autostart" => toggle_autostart(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// 开机自启开关：更新插件状态并持久化到 settings.json，然后重建托盘（刷新勾选态）。
fn toggle_autostart(app: &AppHandle) {
    let on = {
        let st = app.state::<AppState>();
        let mut s = st.settings.lock().unwrap();
        s.autostart = !s.autostart;
        s.save(&st.settings_file);
        s.autostart
    };
    if on {
        if let Err(e) = app.autolaunch().enable() {
            log(app, &format!("开机自启开启失败：{}", e));
        } else {
            log(app, "开机自启已开启");
        }
    } else if let Err(e) = app.autolaunch().disable() {
        log(app, &format!("开机自启关闭失败：{}", e));
    } else {
        log(app, "开机自启已关闭");
    }
    // 重建托盘刷新勾选态
    let _ = app.remove_tray_by_id(TRAY_ID);
    if let Err(e) = build_tray(app) {
        log(app, &format!("重建托盘失败：{}", e));
    }
}

/* ---------------- 原生菜单 ---------------- */

fn build_view_menu(app: &AppHandle) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    let to_panel = MenuItem::with_id(app, "view-panel", "打开环境面板", true, Some("CmdOrCtrl+Shift+P"))?;
    let to_editor = MenuItem::with_id(app, "view-editor", "打开代码编辑器", true, Some("CmdOrCtrl+Shift+E"))?;
    let to_harness = MenuItem::with_id(app, "view-harness", "进入主界面", true, Some("CmdOrCtrl+Shift+H"))?;
    let view = Submenu::with_items(app, "视图", true, &[&to_panel, &to_editor, &to_harness])?;

    let zoom_in = MenuItem::with_id(app, "zoom-in", "放大", true, Some("CmdOrCtrl+="))?;
    let zoom_out = MenuItem::with_id(app, "zoom-out", "缩小", true, Some("CmdOrCtrl+-"))?;
    let zoom_reset = MenuItem::with_id(app, "zoom-reset", "重置缩放", true, Some("CmdOrCtrl+0"))?;
    let reload = MenuItem::with_id(app, "reload", "重新加载", true, Some("CmdOrCtrl+R"))?;
    let devtools = MenuItem::with_id(app, "devtools", "切换开发者工具", true, Some("CmdOrCtrl+Shift+I"))?;
    let win_menu = Submenu::with_items(app, "窗口", true, &[&zoom_in, &zoom_out, &zoom_reset, &reload, &devtools])?;

    let open_log = MenuItem::with_id(app, "open-log", "打开日志目录", true, None::<&str>)?;
    let help = Submenu::with_items(app, "帮助", true, &[&open_log])?;

    Menu::with_items(app, &[&view, &win_menu, &help])
}

/* ---------------- 更新检查 ---------------- */

/// 极简 HTTP GET（仅 http://，本地服务无 TLS），解析 JSON 返回。
fn http_get_json(url: &str) -> Option<serde_json::Value> {
    let rest = url.strip_prefix("http://")?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (hostport, 80),
    };
    let addr = format!("{}:{}", host, port).parse::<std::net::SocketAddr>().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).ok()?;
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: dsh-desktop-host\r\nConnection: close\r\n\r\n",
        path, hostport
    );
    s.write_all(req.as_bytes()).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(3000))).ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1)?;
    serde_json::from_str(body).ok()
}

/// 轮询一次 /api/runtime，有新版本则更新托盘 tooltip 并打日志。
fn check_updates_now(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let port = service_port(&handle);
        let url = format!("http://127.0.0.1:{}/api/runtime", port);
        let state = http_get_json(&url);
        let (update, latest, installed) = state
            .as_ref()
            .map(|j| {
                (
                    j.get("updateAvailable").and_then(|v| v.as_bool()).unwrap_or(false),
                    j.get("latest").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    j.get("installed").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                )
            })
            .unwrap_or((false, String::new(), String::new()));
        if update {
            let tip = format!("DSH Desktop — 发现新版本 {}（当前 {}），可在面板更新", latest, installed);
            if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                let _ = tray.set_tooltip(Some(&tip));
            }
            log(&handle, &tip);
            emit(&handle, "update", &format!("dsh 有新版本 {} 可更新", latest), "可在面板（/panel）一键更新");
        } else {
            let tip = if latest.is_empty() {
                "DSH Desktop — 服务未就绪".to_string()
            } else {
                format!("DSH Desktop — dsh {}（已是最新）", installed)
            };
            if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                let _ = tray.set_tooltip(Some(&tip));
            }
        }
    });
}

/// 后台更新检查：启动 2 分钟后首次，之后每 6 小时一次（对标主流桌面客户端的更新节奏）。
fn start_update_poller(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(120));
        loop {
            check_updates_now(&handle);
            std::thread::sleep(Duration::from_secs(6 * 60 * 60));
        }
    });
}

/* ---------------- 窗口状态 ---------------- */

fn restore_window_bounds<'a, M: tauri::Manager<tauri::Wry>>(
    app: &AppHandle,
    builder: tauri::WebviewWindowBuilder<'a, tauri::Wry, M>,
) -> tauri::WebviewWindowBuilder<'a, tauri::Wry, M> {
    let st = app.state::<AppState>();
    let s = st.settings.lock().unwrap();
    match (s.win_x, s.win_y, s.win_w, s.win_h) {
        (Some(x), Some(y), Some(w), Some(h)) => builder
            .position(x, y)
            .inner_size(w.max(900.0), h.max(620.0)),
        _ => builder,
    }
}

fn save_window_state(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
            let st = app.state::<AppState>();
            let mut s = st.settings.lock().unwrap();
            s.win_x = Some(pos.x as f64);
            s.win_y = Some(pos.y as f64);
            s.win_w = Some(size.width as f64);
            s.win_h = Some(size.height as f64);
            s.save(&st.settings_file);
        }
    }
}

/* ---------------- 启动参数 ---------------- */

/// 启动参数（由 dsh-desktop-tools 插件或命令行传入）：
///   --url <url>       服务已由外部启动，宿主直接打开该地址的窗口
///   --serve           宿主自己启动（或连接）dsh web 服务（默认行为）
///   --port <n>        （--serve 时）服务端口，覆盖 settings.json
///   --workspace <dir> （--serve 时）工作区，覆盖 settings.json
///   --home <dir>      DSH_HOME，覆盖环境变量
#[derive(Default)]
struct LaunchOpts {
    url: Option<String>,
    serve: bool,
    port: Option<u16>,
    workspace: Option<String>,
    home: Option<String>,
}

fn parse_args() -> LaunchOpts {
    let mut opts = LaunchOpts {
        serve: true,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--url" => {
                opts.url = args.next();
                if opts.url.is_some() {
                    opts.serve = false;
                }
            }
            "--serve" => opts.serve = true,
            "--port" => opts.port = args.next().and_then(|s| s.parse().ok()),
            "--workspace" => opts.workspace = args.next(),
            "--home" => opts.home = args.next(),
            _ => {}
        }
    }
    opts
}

async fn bootstrap(app: AppHandle, opts: LaunchOpts) {
    if let Some(h) = &opts.home {
        std::env::set_var("DSH_HOME", h);
    }

    let target = if let Some(u) = opts.url {
        app.state::<AppState>()
            .current_url
            .lock()
            .unwrap()
            .replace(u.clone());
        u
    } else {
        // --serve：应用命令行覆盖，然后启动（或连接）服务
        {
            let st = app.state::<AppState>();
            let mut s = st.settings.lock().unwrap();
            if let Some(p) = opts.port {
                s.port = p;
            }
            if let Some(w) = opts.workspace {
                s.workspace = w;
            }
        }
        match server::start_or_connect(&app).await {
            Ok(url) => url,
            Err(e) => {
                log(&app, &format!("启动失败：{}", e));
                return;
            }
        }
    };

    if let Some(win) = app.get_webview_window("main") {
        if let Ok(u) = tauri::Url::parse(&target) {
            let _ = win.navigate(u);
        }
    }
    emit_ready(&app, &target);
    check_updates_now(&app);
}

/* ---------------- 导航策略 ---------------- */

/// WebView 导航策略：本地地址放行，其余 http(s) 外链交给系统浏览器打开。
fn handle_webview_navigation(url: &tauri::Url) -> bool {
    let is_local = url.scheme() == "about"
        || matches!(
            url.host_str(),
            Some("127.0.0.1" | "localhost" | "tauri.localhost")
        );
    if is_local {
        return true;
    }
    if url.scheme() == "http" || url.scheme() == "https" {
        let _ = opener::open(url.as_str());
    }
    false
}

/* ---------------- 应用入口 ---------------- */

pub fn run() {
    let opts = parse_args();

    let app = tauri::Builder::default()
        // 单实例：重复启动时聚焦已有主窗口，避免产生多个服务进程
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        // 开机自启（托盘开关控制）
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .setup(move |app| {
            let user_data = app.path().app_data_dir().expect("无法获取应用数据目录");
            let _ = std::fs::create_dir_all(&user_data);
            let settings_file = user_data.join("settings.json");
            let log_dir = user_data.clone();
            let runtime_dir = user_data.join("runtime");
            let settings = settings::Settings::load(&settings_file);
            app.manage(AppState {
                settings: Mutex::new(settings),
                server_child: Mutex::new(None),
                parsed_url: Mutex::new(None),
                current_url: Mutex::new(None),
                current_view: Mutex::new("harness".to_string()),
                zoom: Mutex::new(1.0),
                was_ready: AtomicBool::new(false),
                restart_tries: AtomicU32::new(0),
                log_dir,
                runtime_dir,
                settings_file,
            });

            // 同步开机自启状态到系统（settings.json 为真时保证已注册）
            {
                let st = app.state::<AppState>();
                if st.settings.lock().unwrap().autostart {
                    let _ = app.autolaunch().enable();
                }
            }

            // 原生菜单（视图 / 窗口 / 帮助）
            if let Ok(menu) = build_view_menu(app.handle()) {
                let _ = app.set_menu(menu);
            }

            // 系统托盘
            if let Err(e) = build_tray(app.handle()) {
                log(app.handle(), &format!("创建系统托盘失败：{}", e));
            }

            // 主窗口：先加载占位页，bootstrap 后 navigate 到 dsh web 地址
            let builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("DSH Desktop")
            .inner_size(1240.0, 820.0)
            .min_inner_size(900.0, 620.0)
            .center()
            .on_navigation(|url| handle_webview_navigation(url))
            .on_new_window(|url, _features| {
                let _ = opener::open(url.as_str());
                tauri::webview::NewWindowResponse::<tauri::Wry>::Deny
            });
            let builder = restore_window_bounds(app.handle(), builder);
            let _window = builder.build().expect("创建主窗口失败");

            server::supervise(app.handle());

            // 后台更新检查（每 6 小时）
            start_update_poller(app.handle());

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { bootstrap(handle, opts).await });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 tauri 应用失败");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(st) = handle.try_state::<AppState>() {
                if let Some(mut child) = st.server_child.lock().unwrap().take() {
                    let _ = child.kill();
                }
            }
            save_window_state(handle);
        }
        // 菜单：打开面板 / 编辑器 / 主界面；窗口缩放与调试
        if let tauri::RunEvent::MenuEvent(ref event) = event {
            match event.id().as_ref() {
                "view-panel" => switch_view(handle, "panel"),
                "view-editor" => switch_view(handle, "editor"),
                "view-harness" => switch_view(handle, "harness"),
                "zoom-in" => {
                    let st = handle.state::<AppState>();
                    let mut z = st.zoom.lock().unwrap();
                    *z = (*z + 0.1).min(3.0);
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.set_zoom(*z);
                    }
                }
                "zoom-out" => {
                    let st = handle.state::<AppState>();
                    let mut z = st.zoom.lock().unwrap();
                    *z = (*z - 0.1).max(0.2);
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.set_zoom(*z);
                    }
                }
                "zoom-reset" => {
                    let st = handle.state::<AppState>();
                    let mut z = st.zoom.lock().unwrap();
                    *z = 1.0;
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.set_zoom(*z);
                    }
                }
                "reload" => {
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.eval("location.reload()");
                    }
                }
                "devtools" => {
                    if let Some(win) = handle.get_webview_window("main") {
                        if win.is_devtools_open() {
                            let _ = win.close_devtools();
                        } else {
                            let _ = win.open_devtools();
                        }
                    }
                }
                "open-log" => {
                    let st = handle.state::<AppState>();
                    let _ = opener::open(&st.log_dir);
                }
                _ => {}
            }
        }
        if let tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { .. },
            ..
        } = event
        {
            if label == "main" {
                save_window_state(handle);
                handle.exit(0);
            }
        }
    });
}
