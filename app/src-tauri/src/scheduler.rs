//! 확인 루프: 1시간마다 전체 확인 + 리셋권별 정각 만료 알림 + 절전 복귀 감지.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::accounts::BoxFut;
use crate::fetch::{FetchError, Fetched};
use crate::kst::{fmt_kst, kst_day};
use crate::model::{Note, Service};
use crate::rules::{expiry_notes, next_alert_at, on_fetch, total};
use crate::store::Store;

const CHECK_EVERY: StdDuration = StdDuration::from_secs(60 * 60);
const MAX_SLEEP: StdDuration = StdDuration::from_secs(5 * 60);
const WAKE_JUMP: chrono::Duration = chrono::Duration::minutes(2);
const ACCOUNT_TIMEOUT: StdDuration = StdDuration::from_secs(150);

pub trait Fetcher: Send + Sync {
    fn fetch<'a>(&'a self, svc: Service, dir: &'a Path) -> BoxFut<'a, Result<Fetched, FetchError>>;
}

pub trait Sink: Send + Sync {
    fn toast(&self, svc: Service, note: &Note);
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// 실제 조회: Claude는 usage API, Codex는 app-server
pub struct CliFetcher;
impl Fetcher for CliFetcher {
    fn fetch<'a>(&'a self, svc: Service, dir: &'a Path) -> BoxFut<'a, Result<Fetched, FetchError>> {
        Box::pin(async move {
            match svc {
                Service::Claude => crate::claude::fetch(dir).await,
                Service::Codex => crate::codex::fetch(dir).await,
            }
        })
    }
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

impl Sink for crate::notify::Notifier {
    fn toast(&self, svc: Service, note: &Note) {
        if let Err(e) = crate::notify::Notifier::toast(self, svc, note) {
            log::warn!("알림 표시 실패: {e}");
        }
    }
}

/// 화면 계정 한 줄
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: Uuid,
    pub service: Service,
    pub label: String,
    pub alias: Option<String>,
    pub email: Option<String>,
    pub count: u32,
    pub earliest: Option<String>,
    /// "ok" | "auth" | "error" | "unknown"
    pub status: String,
    pub error: Option<String>,
    pub last_ok: Option<String>,
}

#[derive(Debug, Clone)]
enum Status {
    Ok,
    Auth,
    Error(String),
}

pub struct Checker {
    pub store: Store,
    lock: tokio::sync::Mutex<()>,
    status: Mutex<HashMap<Uuid, Status>>,
    fetcher: Arc<dyn Fetcher>,
    sink: Arc<dyn Sink>,
    clock: Arc<dyn Clock>,
}

impl Checker {
    pub fn new(store: Store, fetcher: Arc<dyn Fetcher>, sink: Arc<dyn Sink>, clock: Arc<dyn Clock>) -> Checker {
        Checker { store, lock: tokio::sync::Mutex::new(()), status: Mutex::new(HashMap::new()), fetcher, sink, clock }
    }

    pub async fn check_all(&self) -> Vec<AccountView> {
        let _guard = self.lock.lock().await;
        let mut state = self.store.load_state();
        for a in self.store.load_accounts() {
            let dir = self.store.account_dir(&a);
            let result = tokio::time::timeout(ACCOUNT_TIMEOUT, self.fetcher.fetch(a.service, &dir))
                .await
                .unwrap_or_else(|_| Err(FetchError::Other("시간 초과".into())));
            let now = self.clock.now();
            let st = state.entry(a.id).or_default();
            let label = a.label();
            let mut notes = Vec::new();
            let status = match result {
                Ok(f) => {
                    notes = on_fetch(st, a.service, &label, &f.grants, now);
                    Status::Ok
                }
                Err(FetchError::Auth) => {
                    let today = kst_day(now);
                    if st.auth_fail_day.as_deref() != Some(today.as_str()) {
                        st.auth_fail_day = Some(today);
                        notes.push(Note {
                            title: format!("{}({label}) 재인증 필요", a.service.display()),
                            body: "앱에서 [재인증]을 눌러 다시 로그인해 주세요.".into(),
                        });
                    }
                    notes.extend(expiry_notes(st, a.service, &label, now));
                    Status::Auth
                }
                Err(e) => {
                    log::warn!("{label}: {e}");
                    notes.extend(expiry_notes(st, a.service, &label, now));
                    Status::Error(e.to_string())
                }
            };
            self.status.lock().unwrap().insert(a.id, status);
            for n in &notes {
                log::info!("알림: {} | {}", n.title, n.body.replace('\n', " / "));
                self.sink.toast(a.service, n);
            }
        }
        if let Err(e) = self.store.save_state(&state) {
            log::error!("state 저장 실패: {e}");
        }
        self.views_from(&state)
    }

    /// 캐시 기준으로 지금 해당하는 만료 알림만 보낸다 (조회 없음)
    pub async fn fire_due(&self) {
        let _guard = self.lock.lock().await;
        let now = self.clock.now();
        let mut state = self.store.load_state();
        let mut changed = false;
        for a in self.store.load_accounts() {
            let Some(st) = state.get_mut(&a.id) else { continue };
            for n in expiry_notes(st, a.service, &a.label(), now) {
                changed = true;
                log::info!("알림: {} | {}", n.title, n.body);
                self.sink.toast(a.service, &n);
            }
        }
        if changed {
            if let Err(e) = self.store.save_state(&state) {
                log::error!("state 저장 실패: {e}");
            }
        }
    }

    pub async fn next_wake(&self) -> Option<DateTime<Utc>> {
        let _guard = self.lock.lock().await;
        let now = self.clock.now();
        self.store.load_state().values().filter_map(|st| next_alert_at(st, now)).min()
    }

    pub async fn views(&self) -> Vec<AccountView> {
        let _guard = self.lock.lock().await;
        self.views_from(&self.store.load_state())
    }

    fn views_from(&self, state: &HashMap<Uuid, crate::model::AccountState>) -> Vec<AccountView> {
        let status = self.status.lock().unwrap();
        self.store
            .load_accounts()
            .into_iter()
            .map(|a| {
                let st = state.get(&a.id);
                let cache = st.map(|s| s.cache.as_slice()).unwrap_or_default();
                let (status, error) = match status.get(&a.id) {
                    Some(Status::Ok) => ("ok", None),
                    Some(Status::Auth) => ("auth", None),
                    Some(Status::Error(e)) => ("error", Some(e.clone())),
                    None => ("unknown", None),
                };
                AccountView {
                    id: a.id,
                    service: a.service,
                    label: a.label(),
                    alias: a.alias.clone(),
                    email: a.email.clone(),
                    count: total(cache),
                    earliest: cache.iter().filter_map(|g| g.expires_at).min().map(fmt_kst),
                    status: status.into(),
                    error,
                    last_ok: st.and_then(|s| s.last_ok).map(fmt_kst),
                }
            })
            .collect()
    }
}

/// 백그라운드 루프. `on_change`는 확인이 끝날 때마다 호출된다 (UI 갱신용).
pub fn spawn_loops(checker: Arc<Checker>, on_change: Arc<dyn Fn(Vec<AccountView>) + Send + Sync>) {
    let (c, cb) = (checker.clone(), on_change.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            cb(c.check_all().await);
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
    tauri::async_runtime::spawn(async move {
        loop {
            let before = Utc::now();
            let until_alert = checker.next_wake().await.and_then(|t| (t - before).to_std().ok());
            let nap = until_alert.map_or(MAX_SLEEP, |d| d.min(MAX_SLEEP)) + StdDuration::from_millis(500);
            tokio::time::sleep(nap).await;
            let elapsed = Utc::now() - before;
            if elapsed > chrono::Duration::from_std(nap).unwrap_or_default() + WAKE_JUMP {
                log::info!("절전 복귀 감지 → 전체 확인");
                on_change(checker.check_all().await);
            } else {
                checker.fire_due().await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::BoxFut;
    use crate::fetch::{FetchError, Fetched};
    use crate::model::{Grant, Note, Service};
    use crate::store::{Account, Store};
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 23, 1, 0, 0).unwrap()
    }

    struct FakeFetch {
        by_dir: Mutex<HashMap<String, Result<Fetched, FetchError>>>,
    }
    impl Fetcher for FakeFetch {
        fn fetch<'a>(&'a self, _svc: Service, dir: &'a Path) -> BoxFut<'a, Result<Fetched, FetchError>> {
            let key = dir.file_name().unwrap().to_string_lossy().to_string();
            let r = self.by_dir.lock().unwrap().get(&key).cloned().unwrap_or(Err(FetchError::Other("없음".into())));
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                r
            })
        }
    }

    #[derive(Default)]
    struct FakeSink(Mutex<Vec<Note>>);
    impl Sink for FakeSink {
        fn toast(&self, _svc: Service, note: &Note) {
            self.0.lock().unwrap().push(note.clone());
        }
    }

    struct FakeClock(Mutex<DateTime<Utc>>);
    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        checker: Arc<Checker>,
        fetch: Arc<FakeFetch>,
        sink: Arc<FakeSink>,
        clock: Arc<FakeClock>,
        accounts: Vec<Account>,
    }

    impl Rig {
        fn new(n: usize) -> Rig {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path().to_path_buf());
            let accounts: Vec<Account> = (0..n)
                .map(|i| Account {
                    id: uuid::Uuid::new_v4(),
                    service: Service::Codex,
                    alias: Some(format!("계정{i}")),
                    email: Some(format!("{i}@x")),
                    identity: Some(format!("id{i}")),
                    added_at: t0(),
                })
                .collect();
            store.save_accounts(&accounts).unwrap();
            let fetch = Arc::new(FakeFetch { by_dir: Mutex::new(HashMap::new()) });
            let sink = Arc::new(FakeSink::default());
            let clock = Arc::new(FakeClock(Mutex::new(t0())));
            let checker = Arc::new(Checker::new(store, fetch.clone(), sink.clone(), clock.clone()));
            Rig { _dir: dir, checker, fetch, sink, clock, accounts }
        }

        fn set(&self, i: usize, r: Result<Fetched, FetchError>) {
            let key = self.checker.store.account_dir(&self.accounts[i]).file_name().unwrap().to_string_lossy().to_string();
            self.fetch.by_dir.lock().unwrap().insert(key, r);
        }

        fn advance(&self, h: i64) {
            *self.clock.0.lock().unwrap() = t0() + Duration::hours(h);
        }

        fn titles(&self) -> Vec<String> {
            self.sink.0.lock().unwrap().drain(..).map(|n| n.title).collect()
        }
    }

    fn ok(hours_left: &[i64]) -> Result<Fetched, FetchError> {
        let grants = hours_left
            .iter()
            .enumerate()
            .map(|(i, h)| Grant { id: format!("g{i}"), title: "Full reset".into(), expires_at: Some(t0() + Duration::hours(*h)), count: 1 })
            .collect();
        Ok(Fetched { grants, email: None, identity: None })
    }

    #[tokio::test]
    async fn concurrent_checks_alert_once() {
        let r = Rig::new(1);
        r.set(0, ok(&[100]));
        let (a, b) = (r.checker.clone(), r.checker.clone());
        tokio::join!(a.check_all(), b.check_all());
        assert_eq!(r.titles(), ["Codex(계정0) 리셋권 1개 보유 중"]);
    }

    #[tokio::test]
    async fn auth_failure_still_sends_cached_expiry_and_daily_auth_note() {
        let r = Rig::new(1);
        r.set(0, ok(&[100]));
        r.checker.check_all().await;
        r.titles();
        r.set(0, Err(FetchError::Auth));
        r.advance(80);
        let views = r.checker.check_all().await;
        assert_eq!(r.titles(), ["Codex(계정0) 재인증 필요", "Codex(계정0) 리셋권 24시간 남음"]);
        assert_eq!(views[0].status, "auth");
        r.advance(81);
        r.checker.check_all().await;
        assert!(r.titles().is_empty());
    }

    #[tokio::test]
    async fn failing_account_does_not_block_next() {
        let r = Rig::new(2);
        r.set(0, Err(FetchError::Other("시간 초과".into())));
        r.set(1, ok(&[100]));
        let views = r.checker.check_all().await;
        assert_eq!(r.titles(), ["Codex(계정1) 리셋권 1개 보유 중"]);
        assert_eq!((views[0].status.as_str(), views[1].status.as_str()), ("error", "ok"));
        assert_eq!(views[0].error.as_deref(), Some("시간 초과"));
    }

    #[tokio::test]
    async fn fire_due_after_clock_jump_sends_one_24h() {
        let r = Rig::new(1);
        r.set(0, ok(&[100]));
        r.checker.check_all().await;
        r.titles();
        r.advance(80);
        r.checker.fire_due().await;
        assert_eq!(r.titles(), ["Codex(계정0) 리셋권 24시간 남음"]);
        r.checker.fire_due().await;
        assert!(r.titles().is_empty());
    }

    #[tokio::test]
    async fn next_wake_is_earliest_pending_alert() {
        let r = Rig::new(2);
        r.set(0, ok(&[100]));
        r.set(1, ok(&[90]));
        r.checker.check_all().await;
        assert_eq!(r.checker.next_wake().await, Some(t0() + Duration::hours(18)));
    }

    #[tokio::test]
    async fn view_shows_count_and_earliest() {
        let r = Rig::new(1);
        r.set(0, ok(&[300, 150]));
        let v = r.checker.check_all().await;
        assert_eq!(v[0].count, 2);
        assert_eq!(v[0].earliest.as_deref(), Some("10/29(목) 16:00"));
        assert_eq!(v[0].label, "계정0");
    }
}
