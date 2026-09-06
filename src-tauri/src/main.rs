#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DSH Desk —— 极简壳：
//! - 不改 DSH 网页 UI，所有功能由 DSH 插件生态提供
//! - 自包含：内置 node.exe + dsh 运行时（sidecar + resources）
//! - 自动更新：GitHub Releases + tauri-plugin-updater
//!
//! 启动流程：spawn `node <resources>/dsh-runtime/.../bin.js web --no-open --port 0`
//! → 解析 stdout 中的 `http://127.0.0.1:<port>` → 等端口就绪 → 显示窗口并跳转。
//! 退出/更新前用 taskkill /T /F 杀掉整棵进程树（dsh 会再 spawn 孙进程）。

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use regex::Regex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_shell::process::{CommandEvent, CommandChild};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_updater::UpdaterExt;

/// 安装后位于 resource_dir 下的 dsh 运行时入口（npm install 产物）。
const DSH_ENTRY: &str = "dsh-runtime/node_modules/@deepseek-ai/dsh/lib/bin.js";
/// 现役数据目录（沿用旧壳遗产，插件/凭证/会话全部在此）。
const HARNESS_HOME_SUBPATH: &str = "dsh-desktop\\harness";

struct SidecarState {
    child: Mutex<Option<CommandChild>>,
}

fn harness_home() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|appdata| PathBuf::from(appdata).join(HARNESS_HOME_SUBPATH))
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// 杀掉 sidecar 及其整棵进程树（幂等）。
fn kill_sidecar(app: &AppHandle) {
    let taken = app.state::<SidecarState>().child.lock().unwrap().take();
    if let Some(child) = taken {
        let pid = child.pid();
        let _ = child.kill();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .status();
        }
    }
}

/// 显示主窗口；带 port 时先把窗口导航到本地 DSH 服务。
fn show_main(app: &AppHandle, port: Option<u16>) {
    if let Some(win) = app.get_webview_window("main") {
        if let Some(p) = port {
            let _ = win.eval(&format!("location.replace('http://127.0.0.1:{p}/')"));
        }
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn wait_ready(port: u16, tries: u16) -> bool {
    for _ in 0..tries {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

async fn start_dsh(app: &AppHandle) {
    let resource_dir = match app.path().resource_dir() {
        Ok(p) => p,
        Err(e) => {
            notify(app, "DSH 启动失败", &format!("无法定位资源目录: {e}"));
            return;
        }
    };
    let entry = resource_dir.join(DSH_ENTRY).to_string_lossy().to_string();
    let runtime_dir = resource_dir.join("dsh-runtime");

    let mut command = match app.shell().sidecar("node") {
        Ok(c) => c,
        Err(e) => {
            notify(app, "DSH 启动失败", &format!("找不到内置 node: {e}"));
            return;
        }
    };
    command = command
        .args([entry.as_str(), "web", "--no-open", "--port", "0"])
        // 显式指向现役 harness，不依赖外部环境变量碰巧存在
        .env("DSH_HOME", harness_home().unwrap_or_else(|| runtime_dir.clone()));
    // 让 dsh plugin（转发给 pnpm）能用内置 pnpm.exe
    if let Ok(path) = std::env::var("PATH") {
        command = command.env(
            "PATH",
            format!("{};{}", runtime_dir.to_string_lossy(), path),
        );
    }

    let (mut rx, child) = match command.spawn() {
        Ok(v) => v,
        Err(e) => {
            notify(app, "DSH 启动失败", &format!("sidecar spawn 失败: {e}"));
            return;
        }
    };
    app.state::<SidecarState>()
        .child
        .lock()
        .unwrap()
        .replace(child);

    let url_re = Regex::new(r"https?://127\.0\.0\.1:(\d+)").unwrap();
    let mut launched = false;
    while let Some(event) = rx.recv().await {
        let bytes = match event {
            CommandEvent::Stdout(b) | CommandEvent::Stderr(b) => b,
            CommandEvent::Terminated(_) => {
                if !launched {
                    notify(app, "DSH 已退出", "服务进程意外终止，请重新启动应用");
                }
                break;
            }
            _ => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        if !launched {
            if let Some(caps) = url_re.captures(&text) {
                if let Ok(port) = caps[1].parse::<u16>() {
                    if port > 0 && wait_ready(port, 50) {
                        show_main(app, Some(port));
                        launched = true;
                    }
                }
            }
        }
    }
}

/// 检查并安装更新。manual = 用户从托盘触发（无更新/失败也弹通知）。
fn check_update(app: &AppHandle, manual: bool) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let updater = match handle.updater() {
            Ok(u) => u,
            Err(e) => {
                if manual {
                    notify(&handle, "检查更新失败", &e.to_string());
                }
                return;
            }
        };
        match updater.check().await {
            Ok(Some(update)) => {
                let version = update.version.clone();
                notify(&handle, "发现新版本", &format!("正在下载并安装 v{version}…"));
                // node.exe 正在运行会锁文件，安装前必须先杀进程树
                kill_sidecar(&handle);
                match update.download_and_install(|_, _| {}, || {}).await {
                    Ok(()) => {
                        handle.restart();
                    }
                    Err(e) => notify(&handle, "更新失败", &e.to_string()),
                }
            }
            Ok(None) => {
                if manual {
                    notify(&handle, "已是最新版本", "当前没有可用更新");
                }
            }
            Err(e) => {
                if manual {
                    notify(&handle, "检查更新失败", &e.to_string());
                }
            }
        }
    });
}

fn main() {
    tauri::Builder::default()
        // 必须第一个注册：二次启动只聚焦已有实例
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app, None);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(SidecarState {
            child: Mutex::new(None),
        })
        .setup(|app| {
            let show_i = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let upd_i = MenuItem::with_id(app, "check_update", "检查更新", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "完全退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &upd_i, &quit_i])?;

            TrayIconBuilder::with_id("dsh-tray")
                .icon(app.default_window_icon().expect("missing window icon").clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app, None),
                    "check_update" => check_update(app, true),
                    "quit" => {
                        kill_sidecar(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                show_main(app, None);
                            }
                        }
                    }
                })
                .build(app)?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                start_dsh(&handle).await;
            });

            // 启动后静默检查一次更新（非阻塞，不打扰）
            let handle2 = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(10));
                check_update(&handle2, false);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                kill_sidecar(app);
            }
        });
}
