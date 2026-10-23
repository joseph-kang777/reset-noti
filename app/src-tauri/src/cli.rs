//! 공식 CLI(`claude`, `codex`) 실행 도우미. 모든 자식 프로세스는 창 없이, 타임아웃 시 종료.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug)]
pub enum CliError {
    NotInstalled,
    Timeout,
    /// 종료 코드, stderr 끝 500자 (토큰처럼 보이는 문자열은 가림)
    Exit(i32, String),
    Parse(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::NotInstalled => write!(f, "CLI가 설치되어 있지 않음"),
            CliError::Timeout => write!(f, "시간 초과"),
            CliError::Exit(code, err) => write!(f, "종료 코드 {code}: {err}"),
            CliError::Parse(e) => write!(f, "응답 해석 실패: {e}"),
        }
    }
}

/// PATH + PATHEXT로 실행 파일 찾기 (npm shim `codex.cmd` 포함)
pub fn resolve_cli(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

/// 창 없이, stdin 없이, drop 시 kill되는 명령
/// 사용자 환경에서 물려받으면 계정 OAuth 대신 쓰이거나 다른 설정 폴더를 가리키는 변수
const INHERITED_AUTH_ENV: [&str; 7] =
    ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY", "CODEX_API_KEY", "CLAUDE_CONFIG_DIR", "CODEX_HOME"];

/// 창 없이, stdin 없이, drop 시 kill되는 명령. 작업 폴더는 계정 폴더 밖(임시 폴더).
/// 호출하는 쪽이 `CLAUDE_CONFIG_DIR` / `CODEX_HOME`을 다시 지정한다.
pub fn command(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.stdin(Stdio::null()).kill_on_drop(true).current_dir(std::env::temp_dir());
    for k in INHERITED_AUTH_ENV {
        c.env_remove(k);
    }
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// 자식 프로세스 트리를 묶는 Job. drop하면 손자(npm shim → node → CLI)까지 모두 종료.
pub struct JobGuard(#[cfg(windows)] windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl JobGuard {
    fn attach(child: &tokio::process::Child) -> Option<JobGuard> {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::JobObjects::*;
        unsafe {
            let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            let assigned = child.raw_handle().map(|h| AssignProcessToJobObject(job, HANDLE(h as _)));
            if set.is_err() || !matches!(assigned, Some(Ok(()))) {
                let _ = CloseHandle(job);
                return None;
            }
            Some(JobGuard(job))
        }
    }
}

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

// HANDLE은 커널 객체 핸들일 뿐이라 스레드 간 이동이 안전하다
#[cfg(windows)]
unsafe impl Send for JobGuard {}
#[cfg(windows)]
unsafe impl Sync for JobGuard {}

/// spawn + 프로세스 트리를 Job에 묶기
pub fn spawn(cmd: &mut Command) -> std::io::Result<(tokio::process::Child, Option<JobGuard>)> {
    let child = cmd.spawn()?;
    #[cfg(windows)]
    let guard = JobGuard::attach(&child);
    #[cfg(not(windows))]
    let guard = None;
    if guard.is_none() {
        log::warn!("Job 객체 연결 실패 — 하위 프로세스가 남을 수 있음");
    }
    Ok((child, guard))
}

async fn output(mut cmd: Command, timeout: Duration) -> Result<String, CliError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let (child, _job) = match spawn(&mut cmd) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(CliError::NotInstalled),
        Err(e) => return Err(CliError::Exit(-1, e.to_string())),
    };
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => return Err(CliError::Timeout),
        Ok(Err(e)) => return Err(CliError::Exit(-1, e.to_string())),
        Ok(Ok(o)) => o,
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err
            .chars()
            .rev()
            .take(500)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(CliError::Exit(
            out.status.code().unwrap_or(-1),
            mask_secrets(&tail),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn run_json(cmd: Command, timeout: Duration) -> Result<Value, CliError> {
    let text = output(cmd, timeout).await?;
    serde_json::from_str(text.trim()).map_err(|e| CliError::Parse(e.to_string()))
}

pub async fn run_ok(cmd: Command, timeout: Duration) -> Result<(), CliError> {
    output(cmd, timeout).await.map(|_| ())
}

/// ANSI 색 코드 제거 (`ESC [ ... 문자`)
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 40자 이상 이어진 토큰 문자 덩어리를 `***`로 가린다
pub fn mask_secrets(s: &str) -> String {
    let is_tok = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    let mut out = String::new();
    let mut run = String::new();
    for c in s.chars().chain(std::iter::once(' ')) {
        if is_tok(c) {
            run.push(c);
            continue;
        }
        out.push_str(if run.len() >= 40 { "***" } else { &run });
        run.clear();
        out.push(c);
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn unknown_cli_is_none() {
        assert!(resolve_cli("definitely-not-a-cli-xyz").is_none());
    }

    #[test]
    fn known_cli_resolves() {
        assert!(resolve_cli("cmd").is_some());
    }

    #[tokio::test]
    async fn run_json_parses_stdout() {
        let mut c = tokio::process::Command::new("cmd");
        c.raw_arg(r#"/c echo {"a":1}"#);
        assert_eq!(run_json(c, Duration::from_secs(10)).await.unwrap()["a"], 1);
    }

    #[tokio::test]
    async fn run_json_times_out() {
        let mut c = tokio::process::Command::new("ping");
        c.args(["-n", "60", "127.0.0.1"]);
        assert!(matches!(
            run_json(c, Duration::from_secs(1)).await,
            Err(CliError::Timeout)
        ));
    }

    #[tokio::test]
    async fn nonzero_exit_is_exit_error() {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/c", "exit", "3"]);
        assert!(matches!(
            run_json(c, Duration::from_secs(10)).await,
            Err(CliError::Exit(3, _))
        ));
    }

    #[test]
    fn strip_ansi_removes_colors() {
        assert_eq!(strip_ansi("\u{1b}[94mC6ER-XMU2I\u{1b}[0m"), "C6ER-XMU2I");
    }

    fn running_with(marker: &str) -> bool {
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("(Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*{marker}*' -and $_.Name -eq 'PING.EXE' }}).Count")])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim() != "0"
    }

    /// npm shim(cmd.exe → node → 실제 CLI)처럼 손자 프로세스가 있어도 타임아웃 시 전부 종료
    #[tokio::test]
    async fn timeout_kills_grandchildren() {
        let mut c = command(std::path::Path::new("cmd"));
        c.raw_arg("/c ping -n 30 127.0.0.57");
        assert!(matches!(run_json(c, Duration::from_secs(2)).await, Err(CliError::Timeout)));
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(!running_with("127.0.0.57"), "손자 프로세스(ping)가 남아 있음");
    }

    /// 사용자 환경의 API 키가 계정 OAuth 대신 쓰이지 않도록 제거
    #[test]
    fn command_strips_auth_env() {
        let c = command(std::path::Path::new("cmd"));
        let removed: Vec<String> = c.as_std().get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().to_string()).collect();
        for k in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY", "CODEX_API_KEY", "CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
            assert!(removed.contains(&k.to_string()), "{k} not removed: {removed:?}");
        }
    }
}
