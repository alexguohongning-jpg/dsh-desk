#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DSH Desk —— 极简壳：
//! - 不改 DSH 网页 UI，所有功能由 DSH 插件生态提供
//! - 自包含：内置 node.exe + dsh 运行时（sidecar + resources）
//! - 自动更新：GitHub Releases + tauri-plugin-updater
//!
//! 启动流程：spawn `node <resources>/dsh-runtime/.../bin.js web --no-open --port 0`
//! → 壳自选空闲端口传给 dsh → 轮询端口就绪后显示窗口并跳转（不解析 stdout：node 管道下块缓冲）。
//! 退出/更新前用 taskkill /T /F 杀掉整棵进程树（dsh 会再 spawn 孙进程）。
//! sidecar stderr 全量写入 %APPDATA%\dsh-desktop\harness\dsh-desk-sidecar.log；
//! 进程退出时若检测到 harness 锁冲突（task-board 独占锁，双开必崩），
//! 用占用方 PID 给出明确提示，而不是只留一个「拒绝连接」死窗口。

use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

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

    // 壳自己分配空闲端口传给 dsh：node 的 stdout 在管道下是块缓冲，
    // 「dsh web: http://…」URL 行可能长期不 flush，靠解析它拿端口不可靠。
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(0);
    let port_str = port.to_string();

    let mut command = match app.shell().sidecar("node") {
        Ok(c) => c,
        Err(e) => {
            notify(app, "DSH 启动失败", &format!("找不到内置 node: {e}"));
            return;
        }
    };
    command = command
        .args([entry.as_str(), "web", "--no-open", "--port", port_str.as_str()])
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

    // 独立线程轮询端口就绪后显示窗口；主循环只排空子进程输出并检测退出。
    let handle = app.clone();
    std::thread::spawn(move || {
        if wait_ready(port, 150) {
            show_main(&handle, Some(port));
        } else {
            notify(&handle, "DSH 启动超时", "服务未在预期时间内就绪，请重新启动应用");
        }
    });

    // sidecar stderr 全量落日志文件（排障用），同时内存里留尾部 60 行用于死因分析。
    let log_path = harness_home().map(|h| h.join("dsh-desk-sidecar.log"));
    let mut log_file = log_path.as_ref().and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let mut stderr_tail: VecDeque<String> = VecDeque::with_capacity(61);

    while let Some(event) = rx.recv().await {
        match event {
            CommandEvent::Stderr(bytes) => {
                let line = String::from_utf8_lossy(&bytes).to_string();
                if let Some(f) = log_file.as_mut() {
                    let _ = f.write_all(line.as_bytes());
                }
                stderr_tail.push_back(line);
                while stderr_tail.len() > 60 {
                    stderr_tail.pop_front();
                }
            }
            CommandEvent::Terminated(_) => {
                let tail: String = stderr_tail.iter().cloned().collect();
                if let Some(pid) = parse_lock_owner(&tail) {
                    // task-board 等插件对 harness 账本加独占锁：双开必崩，属预期互斥
                    notify(
                        app,
                        "检测到另一个 DSH 实例正在运行",
                        &format!(
                            "数据目录被进程 PID {pid} 占用，DSH 不能双开。\
                            请先退出它（托盘右键「完全退出」，或执行 taskkill /PID {pid} /T /F），再重新打开 DSH Desk。"
                        ),
                    );
                } else {
                    let reason = tail
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("未知原因")
                        .trim()
                        .chars()
                        .take(120)
                        .collect::<String>();
                    notify(
                        app,
                        "DSH 已退出",
                        &format!("服务进程意外终止：{reason}（完整日志见 dsh-desk-sidecar.log）"),
                    );
                }
                break;
            }
            _ => {}
        }
    }
}

/// 从 stderr 中提取 harness 锁占用方的 PID（"… already owned by process 1234"）。
fn parse_lock_owner(stderr: &str) -> Option<u32> {
    let idx = stderr.find("already owned by process")?;
    stderr[idx..]
        .split_whitespace()
        .nth(4)
        .and_then(|s| s.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok())
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
