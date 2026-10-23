//! 계정 추가/재인증/제거. 로그인은 공식 CLI에 계정 전용 디렉터리를 지정해서 맡긴다.

use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::cli;
use crate::fetch::FetchError;
use crate::model::Service;
use crate::store::{Account, Store};

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub enum LoginEvent {
    /// Codex device-auth: 브라우저에서 입력할 코드
    DeviceCode { url: String, code: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AddError {
    NotInstalled,
    Cancelled,
    Timeout,
    /// 이미 추가된 계정의 라벨
    Duplicate(String),
    Failed(String),
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddError::NotInstalled => write!(f, "CLI가 설치되어 있지 않아요"),
            AddError::Cancelled => write!(f, "취소했어요"),
            AddError::Timeout => write!(f, "10분 안에 로그인이 끝나지 않았어요"),
            AddError::Duplicate(l) => write!(f, "이미 추가된 계정이에요 ({l})"),
            AddError::Failed(e) => write!(f, "로그인 실패: {e}"),
        }
    }
}

impl From<FetchError> for AddError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::NotInstalled => AddError::NotInstalled,
            other => AddError::Failed(other.to_string()),
        }
    }
}

pub trait LoginRunner: Send + Sync {
    fn login<'a>(
        &'a self,
        svc: Service,
        dir: &'a Path,
        on_event: &'a (dyn Fn(LoginEvent) + Send + Sync),
    ) -> BoxFut<'a, Result<(), AddError>>;
    /// (email, identity)
    fn identify<'a>(
        &'a self,
        svc: Service,
        dir: &'a Path,
    ) -> BoxFut<'a, Result<(String, String), AddError>>;
}

async fn login_bounded(
    runner: &dyn LoginRunner,
    svc: Service,
    dir: &Path,
    cancel: CancellationToken,
    timeout: Duration,
    on_event: &(dyn Fn(LoginEvent) + Send + Sync),
) -> Result<(String, String), AddError> {
    tokio::select! {
        _ = cancel.cancelled() => return Err(AddError::Cancelled),
        r = tokio::time::timeout(timeout, runner.login(svc, dir, on_event)) => r.map_err(|_| AddError::Timeout)??,
    }
    runner.identify(svc, dir).await
}

pub async fn add(
    store: &Store,
    runner: &dyn LoginRunner,
    svc: Service,
    cancel: CancellationToken,
    timeout: Duration,
    on_event: &(dyn Fn(LoginEvent) + Send + Sync),
) -> Result<Account, AddError> {
    let mut account = Account {
        id: Uuid::new_v4(),
        service: svc,
        alias: None,
        email: None,
        identity: None,
        added_at: Utc::now(),
    };
    let dir = store.account_dir(&account);
    std::fs::create_dir_all(&dir).map_err(|e| AddError::Failed(e.to_string()))?;
    let cleanup = |e: AddError| {
        let _ = std::fs::remove_dir_all(&dir);
        e
    };

    let (email, identity) = login_bounded(runner, svc, &dir, cancel, timeout, on_event)
        .await
        .map_err(cleanup)?;
    let mut accounts = store.load_accounts();
    if let Some(dup) = accounts
        .iter()
        .find(|a| a.identity.as_deref() == Some(identity.as_str()))
    {
        return Err(cleanup(AddError::Duplicate(dup.label())));
    }
    account.email = Some(email);
    account.identity = Some(identity);
    accounts.push(account.clone());
    store
        .save_accounts(&accounts)
        .map_err(|e| cleanup(AddError::Failed(e.to_string())))?;
    Ok(account)
}

pub async fn reauth(
    store: &Store,
    runner: &dyn LoginRunner,
    id: Uuid,
    cancel: CancellationToken,
    timeout: Duration,
    on_event: &(dyn Fn(LoginEvent) + Send + Sync),
) -> Result<(), AddError> {
    let mut accounts = store.load_accounts();
    let account = accounts
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| AddError::Failed("계정을 찾을 수 없어요".into()))?;
    let dir = store.account_dir(account);
    std::fs::create_dir_all(&dir).map_err(|e| AddError::Failed(e.to_string()))?;
    let (email, identity) =
        login_bounded(runner, account.service, &dir, cancel, timeout, on_event).await?;
    account.email = Some(email);
    account.identity = Some(identity);
    store
        .save_accounts(&accounts)
        .map_err(|e| AddError::Failed(e.to_string()))?;

    let mut state = store.load_state();
    if let Some(st) = state.get_mut(&id) {
        st.auth_fail_day = None;
        store
            .save_state(&state)
            .map_err(|e| AddError::Failed(e.to_string()))?;
    }
    Ok(())
}

pub fn remove(store: &Store, id: Uuid) -> io::Result<()> {
    let mut accounts = store.load_accounts();
    if let Some(a) = accounts.iter().find(|a| a.id == id) {
        let dir = store.account_dir(a);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
    }
    accounts.retain(|a| a.id != id);
    store.save_accounts(&accounts)?;
    let mut state = store.load_state();
    if state.remove(&id).is_some() {
        store.save_state(&state)?;
    }
    Ok(())
}

pub fn rename(store: &Store, id: Uuid, alias: Option<String>) -> io::Result<()> {
    let mut accounts = store.load_accounts();
    if let Some(a) = accounts.iter_mut().find(|a| a.id == id) {
        a.alias = alias
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    store.save_accounts(&accounts)
}

/// `codex login --device-auth` 출력에서 URL과 일회용 코드를 뽑는다.
#[derive(Default)]
pub struct DeviceCodeParser {
    url: Option<String>,
    done: bool,
}

impl DeviceCodeParser {
    pub fn feed(&mut self, line: &str) -> Option<LoginEvent> {
        let s = cli::strip_ansi(line);
        let s = s.trim();
        if s.starts_with("https://") {
            self.url = Some(s.to_string());
            return None;
        }
        let is_code = (8..=14).contains(&s.len())
            && s.contains('-')
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-');
        if is_code && !self.done {
            if let Some(url) = &self.url {
                self.done = true;
                return Some(LoginEvent::DeviceCode {
                    url: url.clone(),
                    code: s.to_string(),
                });
            }
        }
        None
    }
}

/// 실제 러너: `claude auth login` / `codex login --device-auth`
pub struct CliLogin;

fn pipe_lines<R: AsyncRead + Unpin + Send + 'static>(
    r: R,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
}

impl LoginRunner for CliLogin {
    fn login<'a>(
        &'a self,
        svc: Service,
        dir: &'a Path,
        on_event: &'a (dyn Fn(LoginEvent) + Send + Sync),
    ) -> BoxFut<'a, Result<(), AddError>> {
        Box::pin(async move {
            let (name, env, args): (_, _, &[&str]) = match svc {
                Service::Claude => ("claude", "CLAUDE_CONFIG_DIR", &["auth", "login"]),
                Service::Codex => ("codex", "CODEX_HOME", &["login", "--device-auth"]),
            };
            let exe = cli::resolve_cli(name).ok_or(AddError::NotInstalled)?;
            let mut cmd = cli::command(&exe);
            cmd.args(args)
                .env(env, dir)
                .current_dir(dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| AddError::Failed(e.to_string()))?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            pipe_lines(child.stdout.take().unwrap(), tx.clone());
            pipe_lines(child.stderr.take().unwrap(), tx);

            let mut parser = DeviceCodeParser::default();
            let mut tail: Vec<String> = Vec::new();
            while let Some(line) = rx.recv().await {
                if let Some(ev) = parser.feed(&line) {
                    on_event(ev);
                }
                tail.push(cli::strip_ansi(&line));
                if tail.len() > 6 {
                    tail.remove(0);
                }
            }
            let status = child
                .wait()
                .await
                .map_err(|e| AddError::Failed(e.to_string()))?;
            if status.success() {
                Ok(())
            } else {
                Err(AddError::Failed(cli::mask_secrets(tail.join(" ").trim())))
            }
        })
    }

    fn identify<'a>(
        &'a self,
        svc: Service,
        dir: &'a Path,
    ) -> BoxFut<'a, Result<(String, String), AddError>> {
        Box::pin(async move {
            match svc {
                Service::Claude => Ok(crate::claude::whoami_dir(dir).await?),
                Service::Codex => {
                    let f = crate::codex::fetch(dir).await?;
                    Ok((f.email.unwrap_or_default(), f.identity.unwrap_or_default()))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountState, Service};
    use crate::store::Store;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    /// 로그인 결과를 미리 정해두는 가짜 러너
    struct Fake {
        login: Mutex<Option<Result<(), AddError>>>, // None = 끝나지 않음(대기)
        who: (String, String),
    }

    impl Fake {
        fn ok(email: &str, identity: &str) -> Self {
            Fake {
                login: Mutex::new(Some(Ok(()))),
                who: (email.into(), identity.into()),
            }
        }
        fn hang() -> Self {
            Fake {
                login: Mutex::new(None),
                who: ("x".into(), "x".into()),
            }
        }
    }

    impl LoginRunner for Fake {
        fn login<'a>(
            &'a self,
            _svc: Service,
            dir: &'a Path,
            on_event: &'a (dyn Fn(LoginEvent) + Send + Sync),
        ) -> BoxFut<'a, Result<(), AddError>> {
            let r = self.login.lock().unwrap().clone();
            Box::pin(async move {
                assert!(dir.exists(), "로그인 전에 계정 디렉터리가 있어야 함");
                on_event(LoginEvent::DeviceCode {
                    url: "https://auth.openai.com/codex/device".into(),
                    code: "AAAA-BBBBB".into(),
                });
                match r {
                    Some(r) => r,
                    None => std::future::pending().await,
                }
            })
        }
        fn identify<'a>(
            &'a self,
            _svc: Service,
            _dir: &'a Path,
        ) -> BoxFut<'a, Result<(String, String), AddError>> {
            let w = self.who.clone();
            Box::pin(async move { Ok(w) })
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path().to_path_buf());
        (d, s)
    }

    fn account_dirs(s: &Store) -> usize {
        std::fs::read_dir(s.root.join("accounts"))
            .map(|r| r.count())
            .unwrap_or(0)
    }

    fn noop(_: LoginEvent) {}

    #[tokio::test]
    async fn successful_add_saves_account_with_email() {
        let (_d, s) = store();
        let a = add(
            &s,
            &Fake::ok("a@b.c", "codex:1:a@b.c"),
            Service::Codex,
            CancellationToken::new(),
            Duration::from_secs(5),
            &noop,
        )
        .await
        .unwrap();
        assert_eq!(a.email.as_deref(), Some("a@b.c"));
        assert_eq!(s.load_accounts().len(), 1);
        assert!(s.account_dir(&a).exists());
    }

    #[tokio::test]
    async fn device_code_event_is_forwarded() {
        let (_d, s) = store();
        let seen = Mutex::new(Vec::new());
        let on = |e: LoginEvent| seen.lock().unwrap().push(e);
        add(
            &s,
            &Fake::ok("a@b.c", "i"),
            Service::Codex,
            CancellationToken::new(),
            Duration::from_secs(5),
            &on,
        )
        .await
        .unwrap();
        assert_eq!(
            seen.lock().unwrap()[0],
            LoginEvent::DeviceCode {
                url: "https://auth.openai.com/codex/device".into(),
                code: "AAAA-BBBBB".into()
            }
        );
    }

    #[tokio::test]
    async fn cancel_removes_dir_and_keeps_list() {
        let (_d, s) = store();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            c2.cancel();
        });
        let r = add(
            &s,
            &Fake::hang(),
            Service::Claude,
            cancel,
            Duration::from_secs(5),
            &noop,
        )
        .await;
        assert_eq!(r.unwrap_err(), AddError::Cancelled);
        assert_eq!(account_dirs(&s), 0);
        assert!(s.load_accounts().is_empty());
    }

    #[tokio::test]
    async fn timeout_removes_dir() {
        let (_d, s) = store();
        let r = add(
            &s,
            &Fake::hang(),
            Service::Claude,
            CancellationToken::new(),
            Duration::from_millis(50),
            &noop,
        )
        .await;
        assert_eq!(r.unwrap_err(), AddError::Timeout);
        assert_eq!(account_dirs(&s), 0);
    }

    #[tokio::test]
    async fn failed_login_removes_dir() {
        let (_d, s) = store();
        let f = Fake {
            login: Mutex::new(Some(Err(AddError::Failed("boom".into())))),
            who: ("x".into(), "x".into()),
        };
        assert_eq!(
            add(
                &s,
                &f,
                Service::Claude,
                CancellationToken::new(),
                Duration::from_secs(5),
                &noop
            )
            .await
            .unwrap_err(),
            AddError::Failed("boom".into())
        );
        assert_eq!(account_dirs(&s), 0);
    }

    #[tokio::test]
    async fn duplicate_identity_is_rejected() {
        let (_d, s) = store();
        let a = add(
            &s,
            &Fake::ok("a@b.c", "same"),
            Service::Codex,
            CancellationToken::new(),
            Duration::from_secs(5),
            &noop,
        )
        .await
        .unwrap();
        rename(&s, a.id, Some("회사".into())).unwrap();
        let r = add(
            &s,
            &Fake::ok("a@b.c", "same"),
            Service::Codex,
            CancellationToken::new(),
            Duration::from_secs(5),
            &noop,
        )
        .await;
        assert_eq!(r.unwrap_err(), AddError::Duplicate("회사".into()));
        assert_eq!(account_dirs(&s), 1);
        assert_eq!(s.load_accounts().len(), 1);
    }

    #[tokio::test]
    async fn remove_deletes_dir_list_and_state() {
        let (_d, s) = store();
        let a = add(
            &s,
            &Fake::ok("a@b.c", "i"),
            Service::Codex,
            CancellationToken::new(),
            Duration::from_secs(5),
            &noop,
        )
        .await
        .unwrap();
        s.save_state(&HashMap::from([(a.id, AccountState::default())]))
            .unwrap();
        remove(&s, a.id).unwrap();
        assert!(!s.account_dir(&a).exists());
        assert!(s.load_accounts().is_empty());
        assert!(s.load_state().is_empty());
    }

    #[tokio::test]
    async fn reauth_clears_auth_fail_day() {
        let (_d, s) = store();
        let a = add(
            &s,
            &Fake::ok("a@b.c", "i"),
            Service::Codex,
            CancellationToken::new(),
            Duration::from_secs(5),
            &noop,
        )
        .await
        .unwrap();
        let st = AccountState {
            auth_fail_day: Some("2026-10-23".into()),
            ..Default::default()
        };
        s.save_state(&HashMap::from([(a.id, st)])).unwrap();
        reauth(
            &s,
            &Fake::ok("a@b.c", "i"),
            a.id,
            CancellationToken::new(),
            Duration::from_secs(5),
            &noop,
        )
        .await
        .unwrap();
        assert_eq!(s.load_state()[&a.id].auth_fail_day, None);
    }

    #[test]
    fn parses_device_code_from_cli_output() {
        let out = "Welcome to Codex [v\u{1b}[90m0.163.0\u{1b}[0m]\n1. Open this link in your browser and sign in to your account\n   \u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n   \u{1b}[94mC6ER-XMU2I\u{1b}[0m\n";
        let mut p = DeviceCodeParser::default();
        let events: Vec<_> = out.lines().filter_map(|l| p.feed(l)).collect();
        assert_eq!(
            events,
            [LoginEvent::DeviceCode {
                url: "https://auth.openai.com/codex/device".into(),
                code: "C6ER-XMU2I".into()
            }]
        );
    }
}
