// JetStream desktop shell: starts the bundled JetStream server (a PyInstaller
// sidecar) on a private localhost port and shows its web UI in a native window,
// with a tray icon so uploads keep running when the window is closed.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, Url, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(120);
const LOG_LINES: usize = 200;

#[derive(Default)]
struct Server {
    child: Mutex<Option<CommandChild>>,
    log: Mutex<VecDeque<String>>,
    exited: AtomicBool,
    tray_hint_shown: AtomicBool,
}

impl Server {
    fn push_log(&self, line: String) {
        let mut log = self.log.lock().unwrap();
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }

    fn log_text(&self) -> String {
        self.log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n")
    }

    fn stop(&self) {
        if let Some(child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// True once the server answers GET /api/version with 200.
fn server_ready(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let Ok(mut stream) = TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET /api/version HTTP/1.0\r\nHost: {addr}\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 16];
    let n = stream.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn show_startup_error(app: &AppHandle, message: &str) {
    let server = app.state::<Server>();
    let log = server.log_text();
    if let Some(window) = app.get_webview_window("main") {
        let script = format!(
            "window.showStartupError && window.showStartupError({}, {});",
            serde_json::to_string(message).unwrap(),
            serde_json::to_string(&log).unwrap()
        );
        let _ = window.eval(&script);
    }
    show_main_window(app);
}

fn start_server(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let port = free_port()?;
    let (mut events, child) = app
        .shell()
        .sidecar("jetstream-server")?
        .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--no-browser"])
        .env("AUTO_OPEN_BROWSER", "false")
        .env("JETSTREAM_DESKTOP_PARENT_PID", std::process::id().to_string())
        .env("PYTHONUNBUFFERED", "1")
        .spawn()?;
    *app.state::<Server>().child.lock().unwrap() = Some(child);

    // Keep the tail of the server output for the startup error page.
    let log_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let server = log_app.state::<Server>();
            match event {
                CommandEvent::Stdout(line) | CommandEvent::Stderr(line) => {
                    server.push_log(String::from_utf8_lossy(&line).trim_end().to_string());
                }
                CommandEvent::Error(err) => server.push_log(format!("error: {err}")),
                CommandEvent::Terminated(payload) => {
                    server.push_log(format!("server exited (code {:?})", payload.code));
                    server.exited.store(true, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    });

    // Wait for the server, then point the window at it.
    let wait_app = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            if server_ready(port) {
                let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
                if let Some(window) = wait_app.get_webview_window("main") {
                    let _ = window.navigate(url);
                }
                return;
            }
            let server = wait_app.state::<Server>();
            if server.exited.load(Ordering::SeqCst) {
                show_startup_error(&wait_app, "The JetStream server stopped while starting.");
                return;
            }
            if started.elapsed() > SERVER_START_TIMEOUT {
                show_startup_error(&wait_app, "The JetStream server did not respond within two minutes.");
                return;
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    });
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open JetStream", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit JetStream (stops uploads)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("NOAA JetStream")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        // Must be registered first: a second launch focuses the running app.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Server::default())
        .setup(|app| {
            build_tray(app.handle())?;
            if let Err(err) = start_server(app.handle()) {
                let handle = app.handle().clone();
                handle.state::<Server>().push_log(format!("Could not launch server: {err}"));
                // Give the loading page a moment to register its error hook.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(800));
                    show_startup_error(&handle, "The bundled JetStream server could not be launched.");
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps JetStream running in the tray so uploads continue.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                    let app = window.app_handle();
                    let server = app.state::<Server>();
                    if !server.tray_hint_shown.swap(true, Ordering::SeqCst) {
                        let _ = app
                            .notification()
                            .builder()
                            .title("JetStream is still running")
                            .body("Uploads continue in the background. Use the tray icon to reopen or quit.")
                            .show();
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building JetStream");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<Server>().stop();
        }
    });
}
