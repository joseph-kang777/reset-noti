//! Codex 리셋권 조회 — 공식 `codex app-server` JSON-RPC (stdio). 토큰 갱신은 Codex가 한다.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

use crate::cli;
pub use crate::fetch::{FetchError, Fetched};
use crate::model::Grant;

const TIMEOUT: Duration = Duration::from_secs(30);

pub async fn fetch(dir: &Path) -> Result<Fetched, FetchError> {
    let exe = cli::resolve_cli("codex").ok_or(FetchError::NotInstalled)?;
    tokio::time::timeout(TIMEOUT, rpc(&exe, dir))
        .await
        .map_err(|_| FetchError::Other("시간 초과".into()))?
}

/// 초기화까지 끝난 app-server 세션. drop되면 Job이 프로세스 트리를 정리한다.
struct Session {
    child: tokio::process::Child,
    _job: Option<cli::JobGuard>,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Session {
    async fn open(exe: &Path, dir: &Path) -> Result<Session, FetchError> {
        let mut cmd = cli::command(exe);
        cmd.arg("app-server")
            .env("CODEX_HOME", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let (mut child, job) =
            cli::spawn(&mut cmd).map_err(|e| FetchError::Other(format!("codex 실행 실패: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut s = Session {
            child,
            _job: job,
            stdin,
            lines,
            next_id: 1,
        };
        s.call(
            "initialize",
            json!({"clientInfo": {"name": "reset-noti", "version": env!("CARGO_PKG_VERSION")}}),
        )
        .await?;
        send(
            &mut s.stdin,
            json!({"jsonrpc": "2.0", "method": "initialized"}),
        )
        .await?;
        Ok(s)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, FetchError> {
        let id = self.next_id;
        self.next_id += 1;
        call(&mut self.stdin, &mut self.lines, id, method, params).await
    }

    async fn close(mut self) {
        let _ = self.child.kill().await;
    }
}

async fn rpc(exe: &Path, dir: &Path) -> Result<Fetched, FetchError> {
    let mut s = Session::open(exe, dir).await?;
    let (email, identity) = parse_account(&s.call("account/read", json!({})).await?)?;
    let grants = parse_rate_limits(&s.call("account/rateLimits/read", Value::Null).await?);
    s.close().await;
    Ok(Fetched {
        grants,
        email: Some(email),
        identity: Some(identity),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeOutcome {
    /// 리셋권을 써서 사용량이 초기화됨 (같은 키로 이미 성공한 경우 포함)
    Reset,
    /// 초기화할 사용량이 없음
    NothingToReset,
    /// 쓸 수 있는 리셋권이 없음
    NoCredit,
}

/// 리셋권 하나를 사용한다. 키가 리셋권마다 고정이라 재시도해도 두 번 쓰이지 않는다.
pub async fn consume(dir: &Path, credit_id: &str) -> Result<ConsumeOutcome, FetchError> {
    let exe = cli::resolve_cli("codex").ok_or(FetchError::NotInstalled)?;
    tokio::time::timeout(TIMEOUT, async {
        let mut s = Session::open(&exe, dir).await?;
        let r = s
            .call(
                "account/rateLimitResetCredit/consume",
                consume_params(credit_id),
            )
            .await;
        s.close().await;
        parse_consume(&r?)
    })
    .await
    .map_err(|_| FetchError::Other("시간 초과".into()))?
}

pub fn consume_params(credit_id: &str) -> Value {
    json!({"creditId": credit_id, "idempotencyKey": format!("reset-noti-auto:{credit_id}")})
}

pub fn parse_consume(v: &Value) -> Result<ConsumeOutcome, FetchError> {
    match v.get("outcome").and_then(Value::as_str) {
        Some("reset" | "alreadyRedeemed") => Ok(ConsumeOutcome::Reset),
        Some("nothingToReset") => Ok(ConsumeOutcome::NothingToReset),
        Some("noCredit") => Ok(ConsumeOutcome::NoCredit),
        other => Err(FetchError::Other(format!(
            "알 수 없는 사용 결과: {other:?}"
        ))),
    }
}

async fn send(stdin: &mut ChildStdin, msg: Value) -> Result<(), FetchError> {
    let line = format!("{msg}\n");
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| FetchError::Other(format!("app-server 쓰기 실패: {e}")))
}

async fn call(
    stdin: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, FetchError> {
    let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if !params.is_null() {
        msg["params"] = params;
    }
    send(stdin, msg).await?;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| FetchError::Other(e.to_string()))?
    {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(err) = v.get("error") {
            let text = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let low = text.to_lowercase();
            if ["auth", "login", "401", "unauthorized", "token"]
                .iter()
                .any(|k| low.contains(k))
            {
                return Err(FetchError::Auth);
            }
            return Err(FetchError::Other(format!(
                "{method}: {}",
                cli::mask_secrets(&text)
            )));
        }
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    }
    Err(FetchError::Other("app-server가 응답 없이 종료됨".into()))
}

pub fn parse_rate_limits(v: &Value) -> Vec<Grant> {
    let credits = v
        .pointer("/rateLimitResetCredits/credits")
        .and_then(Value::as_array);
    credits
        .into_iter()
        .flatten()
        .filter(|c| c.get("status").and_then(Value::as_str) == Some("available"))
        .filter_map(|c| {
            Some(Grant {
                id: c.get("id")?.as_str()?.to_string(),
                title: c
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("리셋")
                    .to_string(),
                expires_at: c
                    .get("expiresAt")
                    .and_then(Value::as_i64)
                    .and_then(|s| DateTime::<Utc>::from_timestamp(s, 0)),
                count: 1,
            })
        })
        .collect()
}

/// (email, identity) — 로그인되어 있지 않으면 Auth
pub fn parse_account(v: &Value) -> Result<(String, String), FetchError> {
    let email = v.pointer("/account/email").and_then(Value::as_str);
    let Some(email) = email else {
        return Err(if v.get("account").is_some_and(|a| !a.is_null()) {
            FetchError::Other("ChatGPT 계정 로그인이 아님".into())
        } else {
            FetchError::Auth
        });
    };
    let acct = v
        .pointer("/workspaceRouting/chatgptAccountId")
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok((email.to_string(), format!("codex:{acct}:{email}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use serde_json::Value;

    fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn parses_only_available_credits() {
        let grants = parse_rate_limits(&fixture("codex_rate_limits.json"));
        assert_eq!(grants.len(), 2);
        assert_eq!(grants[0].id, "RateLimitResetCredit_aaaa1111");
        assert_eq!(grants[0].title, "Full reset");
        assert_eq!(grants[0].count, 1);
        assert_eq!(
            grants[0].expires_at,
            Some(Utc.with_ymd_and_hms(2026, 10, 29, 19, 13, 31).unwrap())
        );
    }

    #[test]
    fn missing_title_becomes_reset() {
        let v = serde_json::json!({"rateLimitResetCredits": {"credits": [{"id": "x", "status": "available"}]}});
        let g = parse_rate_limits(&v);
        assert_eq!((g[0].title.as_str(), g[0].expires_at), ("리셋", None));
    }

    #[test]
    fn null_credits_is_empty() {
        assert!(parse_rate_limits(&serde_json::json!({"rateLimitResetCredits": null})).is_empty());
    }

    #[test]
    fn consume_params_use_credit_and_stable_key() {
        let p = consume_params("RateLimitResetCredit_x");
        assert_eq!(p["creditId"], "RateLimitResetCredit_x");
        assert_eq!(
            p["idempotencyKey"],
            "reset-noti-auto:RateLimitResetCredit_x"
        );
    }

    #[test]
    fn parses_consume_outcomes() {
        let o = |s: &str| parse_consume(&serde_json::json!({ "outcome": s }));
        assert_eq!(o("reset"), Ok(ConsumeOutcome::Reset));
        assert_eq!(o("nothingToReset"), Ok(ConsumeOutcome::NothingToReset));
        assert_eq!(o("noCredit"), Ok(ConsumeOutcome::NoCredit));
        assert_eq!(o("alreadyRedeemed"), Ok(ConsumeOutcome::Reset));
        assert!(o("weird").is_err());
    }

    #[test]
    fn parses_account_email_and_identity() {
        let (email, identity) = parse_account(&fixture("codex_account.json")).unwrap();
        assert_eq!(email, "user@example.com");
        assert_eq!(
            identity,
            "codex:00000000-0000-0000-0000-000000000001:user@example.com"
        );
    }

    #[test]
    fn null_account_is_auth_error() {
        assert!(matches!(
            parse_account(&serde_json::json!({"account": null})),
            Err(FetchError::Auth)
        ));
    }

    /// RESET_NOTI_CODEX_HOME=<로그인된 CODEX_HOME> cargo test codex_live -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn codex_live() {
        let dir = std::env::var("RESET_NOTI_CODEX_HOME").expect("RESET_NOTI_CODEX_HOME");
        let f = fetch(std::path::Path::new(&dir)).await.unwrap();
        println!("{:?} {:?} {:#?}", f.email, f.identity, f.grants);
    }
}
