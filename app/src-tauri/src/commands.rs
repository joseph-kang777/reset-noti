//! 화면에서 호출하는 Tauri 커맨드.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::accounts::{self, CliLogin, LOGIN_TIMEOUT, LoginEvent};
use crate::cli::resolve_cli;
use crate::model::Service;
use crate::scheduler::{AccountView, Checker};
use crate::store::Store;

pub struct AppState {
    pub store: Store,
    pub checker: Arc<Checker>,
    pub login_cancel: Mutex<Option<CancellationToken>>,
}

#[derive(Serialize, Clone)]
struct LoginEventPayload {
    url: String,
    code: String,
}

#[derive(Serialize)]
pub struct CliStatus {
    claude: bool,
    codex: bool,
}

pub fn emit_accounts(app: &AppHandle, views: &[AccountView]) {
    let _ = app.emit("accounts-changed", views);
}

fn login_sink(app: AppHandle) -> impl Fn(LoginEvent) + Send + Sync {
    move |e: LoginEvent| match e {
        LoginEvent::DeviceCode { url, code } => {
            let _ = app.emit("login-event", LoginEventPayload { url, code });
        }
    }
}

fn new_login(state: &AppState) -> CancellationToken {
    let token = CancellationToken::new();
    if let Some(old) = state.login_cancel.lock().unwrap().replace(token.clone()) {
        old.cancel();
    }
    token
}

/// 계정 변경 후 백그라운드로 다시 확인하고 화면 갱신
fn recheck(app: &AppHandle, checker: Arc<Checker>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let views = checker.check_all().await;
        emit_accounts(&app, &views);
    });
}

#[tauri::command]
pub async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<AccountView>, String> {
    Ok(state.checker.views().await)
}

#[tauri::command]
pub async fn check_now(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AccountView>, String> {
    let views = state.checker.check_all().await;
    emit_accounts(&app, &views);
    Ok(views)
}

#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    state: State<'_, AppState>,
    service: Service,
) -> Result<(), String> {
    let token = new_login(&state);
    let sink = login_sink(app.clone());
    accounts::add(
        &state.store,
        &CliLogin,
        service,
        token,
        LOGIN_TIMEOUT,
        &sink,
    )
    .await
    .map_err(|e| e.to_string())?;
    emit_accounts(&app, &state.checker.views().await);
    recheck(&app, state.checker.clone());
    Ok(())
}

#[tauri::command]
pub async fn reauth(app: AppHandle, state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    let token = new_login(&state);
    let sink = login_sink(app.clone());
    let pending = accounts::reauth_login(&state.store, &CliLogin, id, token, LOGIN_TIMEOUT, &sink)
        .await
        .map_err(|e| e.to_string())?;
    let store = state.store.clone();
    state.checker.exclusive(move || pending.commit(&store)).await.map_err(|e| e.to_string())?;
    recheck(&app, state.checker.clone());
    Ok(())
}

#[tauri::command]
pub fn cancel_login(state: State<'_, AppState>) {
    if let Some(t) = state.login_cancel.lock().unwrap().take() {
        t.cancel();
    }
}

#[tauri::command]
pub async fn remove_account(app: AppHandle, state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    let store = state.store.clone();
    state.checker.exclusive(move || accounts::remove(&store, id)).await.map_err(|e| e.to_string())?;
    emit_accounts(&app, &state.checker.views().await);
    Ok(())
}

#[tauri::command]
pub async fn rename_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
    alias: Option<String>,
) -> Result<(), String> {
    accounts::rename(&state.store, id, alias).map_err(|e| e.to_string())?;
    emit_accounts(&app, &state.checker.views().await);
    Ok(())
}

#[tauri::command]
pub fn cli_status() -> CliStatus {
    CliStatus {
        claude: resolve_cli("claude").is_some(),
        codex: resolve_cli("codex").is_some(),
    }
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let al = app.autolaunch();
    if enabled { al.enable() } else { al.disable() }.map_err(|e| e.to_string())
}
