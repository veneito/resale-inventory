//! Desktop app: runs its own Resale Inventory server and shows it in a native window.
//! Server → "Another server…" points the window at a different server instead.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{Menu, MenuItem, Submenu};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

// Where Tauri serves the bundled connect page on each platform.
#[cfg(windows)]
const CONNECT: &str = "http://tauri.localhost/";
#[cfg(not(windows))]
const CONNECT: &str = "tauri://localhost/";

/// Address of the server this app runs, e.g. http://127.0.0.1:8080.
struct LocalUrl(String);
/// True while the connect page is open, so the next page it opens is remembered as the chosen server.
struct Connecting(AtomicBool);

fn remote_file(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .expect("app data dir")
        .join("server.txt")
}

fn main() {
    tauri::Builder::default()
        .manage(Connecting(AtomicBool::new(false)))
        .menu(|app| {
            let local = MenuItem::with_id(app, "local", "This computer", true, None::<&str>)?;
            let other = MenuItem::with_id(app, "other", "Another server…", true, None::<&str>)?;
            Menu::with_items(
                app,
                &[&Submenu::with_items(
                    app,
                    "Server",
                    true,
                    &[&local, &other],
                )?],
            )
        })
        .on_menu_event(|app, event| {
            let Some(w) = app.get_webview_window("main") else {
                return;
            };
            match event.id().as_ref() {
                "local" => {
                    let _ = std::fs::remove_file(remote_file(app));
                    let _ = w.navigate(app.state::<LocalUrl>().0.parse().expect("valid url"));
                }
                "other" => {
                    app.state::<Connecting>().0.store(true, Ordering::SeqCst);
                    let _ = w.navigate(CONNECT.parse().expect("valid url"));
                }
                _ => {}
            }
        })
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // All interfaces so phones on the same Wi-Fi can connect; any free port if 8080 is taken.
            let listener = std::net::TcpListener::bind("0.0.0.0:8080")
                .or_else(|_| std::net::TcpListener::bind("0.0.0.0:0"))?;
            let local = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
            tauri::async_runtime::spawn(resale_inventory::serve(
                listener,
                dir.join("inventory.db"),
                None,
            ));

            let start = std::fs::read_to_string(dir.join("server.txt"))
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| local.clone());
            app.manage(LocalUrl(local));

            let handle = app.handle().clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(start.trim().parse()?))
                .title("Resale Inventory")
                .inner_size(480.0, 820.0)
                .min_inner_size(360.0, 480.0)
                .on_navigation(move |url| {
                    let connecting = &handle.state::<Connecting>().0;
                    if connecting.load(Ordering::SeqCst)
                        && url.scheme().starts_with("http")
                        && url.host_str() != Some("tauri.localhost")
                    {
                        connecting.store(false, Ordering::SeqCst);
                        let _ = std::fs::write(
                            remote_file(&handle),
                            url.origin().ascii_serialization(),
                        );
                    }
                    true
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Resale Inventory");
}
