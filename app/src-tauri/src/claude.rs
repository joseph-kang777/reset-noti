//! Claude 리셋권 조회 — 비공식 `/api/oauth/usage?cedar_ember=1`.
//! 토큰 갱신은 공식 CLI(`claude -p`)에 맡긴다 (스펙 9.5절, B 경로).

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::cli;
pub use crate::fetch::{FetchError, Fetched};
use crate::model::Grant;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1";
const REFRESH_MARGIN_MIN: i64 = 10;

pub async fn fetch(dir: &Path) -> Result<Fetched, FetchError> {
    let exe = cli::resolve_cli("claude").ok_or(FetchError::NotInstalled)?;
    ensure_fresh_with(dir, &exe).await?;
    let (token, _) = read_token(dir)?;
    let resp = reqwest::Client::new()
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(
            "User-Agent",
            concat!("reset-noti/", env!("CARGO_PKG_VERSION")),
        )
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| FetchError::Other(format!("요청 실패: {}", e.without_url())))?;
    let status = resp.status();
    if status == 401 || status == 403 {
        return Err(FetchError::Auth);
    }
    if !status.is_success() {
        return Err(FetchError::Other(format!("HTTP {status}")));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::Other(format!("응답 해석 실패: {e}")))?;
    if v.pointer("/cedar_ember/grants")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        log::info!("claude cedar_ember: {}", v["cedar_ember"]);
    }
    let (email, identity) = whoami(dir, &exe).await?;
    Ok(Fetched {
        grants: parse_usage(&v),
        email: Some(email),
        identity: Some(identity),
    })
}

/// (accessToken, expiresAt) — 파일·토큰이 없으면 Auth
pub fn read_token(dir: &Path) -> Result<(String, Option<DateTime<Utc>>), FetchError> {
    let text =
        std::fs::read_to_string(dir.join(".credentials.json")).map_err(|_| FetchError::Auth)?;
    let v: Value = serde_json::from_str(&text).map_err(|_| FetchError::Auth)?;
    let oauth = &v["claudeAiOauth"];
    let token = oauth["accessToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or(FetchError::Auth)?;
    let expires = oauth["expiresAt"]
        .as_i64()
        .and_then(DateTime::<Utc>::from_timestamp_millis);
    Ok((token.to_string(), expires))
}

fn is_fresh(dir: &Path) -> Result<bool, FetchError> {
    let (_, expires) = read_token(dir)?;
    Ok(expires.map_or(true, |e| {
        e > Utc::now() + chrono::Duration::minutes(REFRESH_MARGIN_MIN)
    }))
}

/// 만료 10분 전이면 `claude -p`로 공식 CLI가 토큰을 갱신하게 한다. 그래도 만료면 Auth.
pub async fn ensure_fresh_with(dir: &Path, exe: &Path) -> Result<(), FetchError> {
    if is_fresh(dir)? {
        return Ok(());
    }
    log::info!("claude 토큰 갱신 시도: {}", dir.display());
    let mut cmd = cli::command(exe);
    cmd.args(["-p", "ok", "--model", "haiku", "--max-turns", "1"])
        .env("CLAUDE_CONFIG_DIR", dir)
        .current_dir(dir);
    if let Err(e) = cli::run_ok(cmd, Duration::from_secs(60)).await {
        log::warn!("claude 토큰 갱신 실패: {e}");
    }
    if is_fresh(dir)? {
        Ok(())
    } else {
        Err(FetchError::Auth)
    }
}

/// (email, identity) — 계정 추가 직후 신원 확인용
pub async fn whoami_dir(dir: &Path) -> Result<(String, String), FetchError> {
    let exe = cli::resolve_cli("claude").ok_or(FetchError::NotInstalled)?;
    whoami(dir, &exe).await
}

async fn whoami(dir: &Path, exe: &Path) -> Result<(String, String), FetchError> {
    let mut cmd = cli::command(exe);
    cmd.args(["auth", "status", "--json"])
        .env("CLAUDE_CONFIG_DIR", dir)
        .current_dir(dir);
    let v = cli::run_json(cmd, Duration::from_secs(30))
        .await
        .map_err(|e| FetchError::Other(format!("claude auth status: {e}")))?;
    parse_whoami(&v)
}

/// (email, identity) — `claude auth status --json`
pub fn parse_whoami(v: &Value) -> Result<(String, String), FetchError> {
    if v["loggedIn"].as_bool() != Some(true) {
        return Err(FetchError::Auth);
    }
    let email = v["email"].as_str().ok_or(FetchError::Auth)?;
    let org = v["orgId"].as_str().unwrap_or("");
    Ok((email.to_string(), format!("claude:{org}:{email}")))
}

/// 스펙 5절·9항: 형태를 확인하지 못해 관대하게 읽는다.
pub fn parse_usage(v: &Value) -> Vec<Grant> {
    let grants = v.pointer("/cedar_ember/grants").and_then(Value::as_array);
    grants
        .into_iter()
        .flatten()
        .filter_map(|raw| match raw {
            Value::String(id) => Some(Grant {
                id: id.clone(),
                title: "리셋".into(),
                expires_at: None,
                count: 1,
            }),
            Value::Object(o) => {
                let id = ["grant_id", "id"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str))?;
                let kind = ["kind", "reset_type", "type"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str));
                let expires = ["expires_at", "expiresAt", "expires", "valid_until"]
                    .iter()
                    .find_map(|k| o.get(*k))
                    .and_then(parse_time);
                let count = o
                    .get("resets_left")
                    .and_then(Value::as_u64)
                    .map_or(1, |n| n as u32);
                Some(Grant {
                    id: id.to_string(),
                    title: kind.map_or("리셋".to_string(), |k| format!("{k} reset")),
                    expires_at: expires,
                    count,
                })
            }
            _ => None,
        })
        .collect()
}

fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
            .ok()
            .map(|d| d.with_timezone(&Utc)),
        Value::Number(n) => {
            let n = n.as_i64()?;
            if n > 1_000_000_000_000 {
                DateTime::from_timestamp_millis(n)
            } else {
                DateTime::from_timestamp(n, 0)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::Value;

    fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn creds_dir(expires_ms: i64) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let body =
            serde_json::json!({"claudeAiOauth": {"accessToken": "tok", "expiresAt": expires_ms}});
        std::fs::write(d.path().join(".credentials.json"), body.to_string()).unwrap();
        d
    }

    #[test]
    fn empty_grants_parse_to_nothing() {
        assert!(parse_usage(&fixture("claude_usage_empty.json")).is_empty());
    }

    #[test]
    fn null_cedar_ember_is_empty() {
        assert!(parse_usage(&serde_json::json!({"cedar_ember": null})).is_empty());
    }

    #[test]
    fn grants_parse_tolerantly() {
        let g = parse_usage(&fixture("claude_usage_grants.json"));
        let got: Vec<_> = g
            .iter()
            .map(|g| (g.id.as_str(), g.title.as_str(), g.count, g.expires_at))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "g1",
                    "weekly reset",
                    2,
                    Some(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap())
                ),
                ("g2", "리셋", 1, None)
            ]
        );
    }

    #[tokio::test]
    async fn fresh_token_runs_no_command() {
        let d = creds_dir((Utc::now() + Duration::hours(5)).timestamp_millis());
        let missing = std::path::Path::new("Z:/no/such/claude.exe");
        assert_eq!(ensure_fresh_with(d.path(), missing).await, Ok(()));
    }

    #[tokio::test]
    async fn expired_token_that_cannot_refresh_is_auth() {
        let d = creds_dir((Utc::now() - Duration::hours(1)).timestamp_millis());
        let missing = std::path::Path::new("Z:/no/such/claude.exe");
        assert_eq!(
            ensure_fresh_with(d.path(), missing).await,
            Err(FetchError::Auth)
        );
    }

    #[test]
    fn missing_credentials_is_auth() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_token(d.path()).unwrap_err(), FetchError::Auth);
    }

    #[test]
    fn parses_whoami() {
        let v = serde_json::json!({"loggedIn": true, "email": "a@b.c", "orgId": "org1"});
        assert_eq!(
            parse_whoami(&v).unwrap(),
            ("a@b.c".to_string(), "claude:org1:a@b.c".to_string())
        );
        assert_eq!(
            parse_whoami(&serde_json::json!({"loggedIn": false})).unwrap_err(),
            FetchError::Auth
        );
    }

    /// RESET_NOTI_CLAUDE_DIR=<로그인된 CLAUDE_CONFIG_DIR> cargo test claude_live -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn claude_live() {
        let dir = std::env::var("RESET_NOTI_CLAUDE_DIR").expect("RESET_NOTI_CLAUDE_DIR");
        let f = fetch(std::path::Path::new(&dir)).await.unwrap();
        println!("{:?} {:?} {:#?}", f.email, f.identity, f.grants);
    }
}
