//! 자동 업데이트: 시작 시 + 24시간마다 확인 → 화면 배너 + 알림. 사용자가 [설치]를 누르면 설치 후 재시작.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

use crate::model::{Note, Service};
use crate::scheduler::Sink;

const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub fn prompt_text(version: &str) -> String {
    format!("새 버전 {version}이(가) 있어요. 지금 설치할까요?")
}

async fn available_version(app: &AppHandle) -> Option<String> {
    match app.updater() {
        Ok(u) => match u.check().await {
            Ok(Some(update)) => Some(update.version),
            Ok(None) => None,
            Err(e) => {
                log::warn!("업데이트 확인 실패: {e}");
                None
            }
        },
        Err(e) => {
            log::warn!("업데이터 초기화 실패: {e}");
            None
        }
    }
}

pub fn spawn(app: AppHandle, sink: Arc<dyn Sink>) {
    tauri::async_runtime::spawn(async move {
        let mut notified: Option<String> = None;
        loop {
            if let Some(v) = available_version(&app).await {
                let _ = app.emit("update-available", &v);
                if notified.as_deref() != Some(v.as_str()) {
                    sink.toast(Service::Codex, &Note { title: "리셋권 알림 업데이트".into(), body: "새 버전이 있어요. 앱을 열어 설치해 주세요.".into() });
                    notified = Some(v);
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// 화면의 [설치] 버튼
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let Some(update) = update else { return Err("이미 최신 버전이에요".into()) };
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    app.restart();
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Option<String> {
    available_version(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_names_version() {
        assert_eq!(prompt_text("0.2.0"), "새 버전 0.2.0이(가) 있어요. 지금 설치할까요?");
    }
}
