//! Vardøger, the app shell.
//!
//! Desktop: the WHOLE twin runs in this process — `twin_runtime::server::serve` on
//! its own thread, the webview pointed at it.  The window is the same thin client
//! the browser gets; nothing here duplicates the UI.
//!
//! Phone (iOS/Android): raw V8 does not cross-compile there, so the app is a thin
//! host — the bundled `web/index.html` with its "Twin node…" form, connecting out
//! to a node you run elsewhere (your desktop app, or `twin serve` on any machine).

use tauri::{WebviewUrl, WebviewWindowBuilder};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let url = node_url(app.handle());
            WebviewWindowBuilder::new(app, "main", url)
                .title("Vardøger")
                .inner_size(1280.0, 860.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Vardøger");
}

/// Desktop: make sure a twin node is listening, then point the window at it.
/// An already-running node (a `twin serve` you started, or another app window's)
/// is simply attached to — one node, any number of faces.
#[cfg(desktop)]
fn node_url(app: &tauri::AppHandle) -> WebviewUrl {
    use std::net::TcpStream;
    let addr = std::env::var("TWIN_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    if TcpStream::connect(&addr).is_err() {
        let home = twin_home(app);
        let _ = std::env::set_current_dir(&home);
        let a = addr.clone();
        std::thread::spawn(move || {
            if let Err(e) = twin_runtime::server::serve(&a) {
                eprintln!("twin node failed: {e}");
            }
        });
        // the window may not open before the node listens
        for _ in 0..200 {
            if TcpStream::connect(&addr).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    WebviewUrl::External(format!("http://{addr}").parse().expect("node address parses as a url"))
}

/// Where this twin lives — the runtime reads `data/` and `skills/` relative to its
/// working directory.  TWIN_HOME wins; a checkout you run from is used in place;
/// otherwise the OS app-data dir becomes the twin's home.
#[cfg(desktop)]
fn twin_home(app: &tauri::AppHandle) -> std::path::PathBuf {
    use tauri::Manager;
    if let Ok(h) = std::env::var("TWIN_HOME") {
        return h.into();
    }
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("data").is_dir() {
            return cwd;
        }
        if cwd.join("..").join("data").is_dir() {
            return cwd.join(".."); // `cargo run` from tauri/ inside the checkout
        }
    }
    let dir = app.path().app_data_dir().expect("an app data dir exists");
    let _ = std::fs::create_dir_all(dir.join("data"));
    let _ = std::fs::create_dir_all(dir.join("skills"));
    dir
}

/// Phone: the bundled thin client; it asks for (and remembers) your node's address.
#[cfg(mobile)]
fn node_url(_app: &tauri::AppHandle) -> WebviewUrl {
    WebviewUrl::App("index.html".into())
}
