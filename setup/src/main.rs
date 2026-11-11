//! The setup.exe of a Jay app: one window around the NSIS installer embedded in this file.
//! Flags: /P (passive: no questions), /S (silent: no window), /UPDATE /R /NS (passed on to the installer).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use jay_i_setup::{Args, busy, install_folder, open_app, parse, payload_args, run_payload, webview2_installed};
use std::sync::atomic::{AtomicI32, Ordering};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

static PAYLOAD: &[u8] = include_bytes!(env!("JAYI_PAYLOAD_PATH"));
static MARK: &[u8] = include_bytes!(env!("JAYI_MARK_PATH"));
const NAME: &str = env!("JAYI_NAME");
const VERSION: &str = env!("JAYI_VERSION");
const EXE: &str = env!("JAYI_EXE");

/// 1 = cancelled, as NSIS reports a user cancel; replaced by the payload's code once it ran.
static EXIT: AtomicI32 = AtomicI32::new(1);

fn file_name() -> String {
    format!("{NAME}_{VERSION}_x64-setup.exe")
}

/// (name, version, installed version, install folder, passive)
type Info = (String, String, Option<String>, String, bool);

#[tauri::command]
fn info(args: State<Args>) -> Info {
    let installed = jay_i::installed(NAME).ok().flatten().map(|i| i.version);
    (NAME.into(), VERSION.into(), installed, install_folder(NAME), args.passive)
}

#[tauri::command]
async fn install(args: State<'_, Args>) -> Result<i32, String> {
    // A second call while one runs must not touch EXIT (run_payload would refuse it anyway).
    if busy() {
        return Err("another install is already running".into());
    }
    let payload_args = payload_args(&args, true);
    let result = tauri::async_runtime::spawn_blocking(move || run_payload(PAYLOAD, &file_name(), &payload_args)).await;
    match result.map_err(|e| e.to_string())? {
        Ok(code) => {
            EXIT.store(code, Ordering::SeqCst);
            Ok(code)
        }
        Err(e) => {
            EXIT.store(2, Ordering::SeqCst);
            Err(e)
        }
    }
}

#[tauri::command]
fn open(app: AppHandle) -> Result<(), String> {
    open_app(NAME, EXE)?;
    EXIT.store(0, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn close(app: AppHandle) {
    app.exit(0);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetDefaultDllDirectories(flags: u32) -> i32;
}

fn main() {
    // LOAD_LIBRARY_SEARCH_SYSTEM32 for every later LoadLibrary by name (the static imports are covered
    // by /DEPENDENTLOADFLAG in build.rs): a setup run from Downloads must not load a planted
    // version.dll, cryptbase.dll or propsys.dll, which the WebView2 start-up asks for by name.
    unsafe { SetDefaultDllDirectories(0x800) };
    let args = parse(std::env::args().skip(1));
    let webview2 = webview2_installed();
    if args.silent || !webview2 {
        let code = run_payload(PAYLOAD, &file_name(), &payload_args(&args, webview2)).unwrap_or(2);
        std::process::exit(code);
    }

    let app = tauri::Builder::default()
        .manage(args)
        .register_uri_scheme_protocol("jayi", |_ctx, request| {
            let (status, body) = if request.uri().path() == "/mark.png" { (StatusCode::OK, MARK) } else { (StatusCode::NOT_FOUND, &[][..]) };
            Response::builder().status(status).header("Content-Type", "image/png").body(body.to_vec()).unwrap_or_default()
        })
        .setup(|app| {
            // Its own profile folder in temp, so no WebView2 data stays in %LOCALAPPDATA%.
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title(NAME)
                .inner_size(480.0, 560.0)
                .resizable(false)
                .maximizable(false)
                .center()
                .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-gpu")
                .data_directory(std::env::temp_dir().join("jay-i-setup-webview"))
                .build()?;
            Ok(())
        })
        .on_window_event(|_window, event| {
            // A silent NSIS run can't be cancelled, so the window stays while it runs.
            if let WindowEvent::CloseRequested { api, .. } = event
                && busy()
            {
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![info, install, open, close])
        .build(tauri::generate_context!())
        .expect("cannot start the setup");
    app.run_return(|_, _| {});
    std::process::exit(EXIT.load(Ordering::SeqCst));
}
