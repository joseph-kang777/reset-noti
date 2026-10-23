pub mod accounts;
pub mod claude;
pub mod cli;
pub mod codex;
pub mod commands;
pub mod fetch;
pub mod kst;
pub mod model;
pub mod notify;
pub mod rules;
pub mod scheduler;
pub mod store;
pub mod updater;

use std::sync::{Arc, Mutex};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt;

use commands::AppState;
use scheduler::{Checker, CliFetcher, SystemClock};
use store::Store;

const MINIMIZED_ARG: &str = "--minimized";

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// 최초 실행 시 자동 시작을 켠다 (이후엔 사용자 설정을 따름)
fn init_autostart(app: &AppHandle, store: &Store) {
    let flag = store.root.join("autostart_initialized");
    if flag.exists() {
        return;
    }
    if let Err(e) = app.autolaunch().enable() {
        log::warn!("자동 시작 설정 실패: {e}");
    }
    let _ = std::fs::create_dir_all(&store.root);
    let _ = std::fs::write(flag, "1");
}

fn setup_tray(app: &AppHandle, checker: Arc<Checker>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "열기", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "지금 확인", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &check, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip(notify::APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "check" => {
                let (app, c) = (app.clone(), checker.clone());
                tauri::async_runtime::spawn(async move {
                    let views = c.check_all().await;
                    commands::emit_accounts(&app, &views);
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = Store::open(Store::default_root());
    let log_dir = store.root.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Folder {
                        path: log_dir,
                        file_name: Some("reset_noti".into()),
                    },
                ))
                .build(),
        )
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args([MINIMIZED_ARG])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let icon_dir = app.path().resource_dir()?.join("icons");
            let notifier = notify::Notifier::new(&app.config().identifier, icon_dir);
            if let Err(e) = notifier.register_app() {
                log::warn!("알림 앱 등록 실패: {e}");
            }
            let notifier = Arc::new(notifier);
            let checker = Arc::new(Checker::new(
                store.clone(),
                Arc::new(CliFetcher),
                notifier.clone(),
                Arc::new(SystemClock),
            ));
            updater::spawn(handle.clone(), notifier);
            init_autostart(&handle, &store);
            setup_tray(&handle, checker.clone())?;
            let emit_handle = handle.clone();
            scheduler::spawn_loops(
                checker.clone(),
                Arc::new(move |views| commands::emit_accounts(&emit_handle, &views)),
            );
            app.manage(AppState {
                store: store.clone(),
                checker,
                login_cancel: Mutex::new(None),
            });
            if !std::env::args().any(|a| a == MINIMIZED_ARG) {
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::check_now,
            commands::add_account,
            commands::reauth,
            commands::cancel_login,
            commands::remove_account,
            commands::rename_account,
            commands::cli_status,
            commands::get_autostart,
            commands::set_autostart,
            updater::install_update,
            updater::check_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
