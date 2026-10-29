use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Claude,
    Codex,
}

impl Service {
    pub fn display(&self) -> &'static str {
        match self {
            Service::Claude => "Claude",
            Service::Codex => "Codex",
        }
    }

    pub fn usage_url(&self) -> &'static str {
        match self {
            Service::Claude => "https://claude.ai/settings/usage",
            Service::Codex => "https://chatgpt.com/codex/settings/usage",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    pub title: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub title: String,
    pub body: String,
}

/// 계정 하나의 알림 상태. state.json에 계정 id별로 저장된다.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountState {
    /// 본 리셋권 id → 마지막으로 본 개수
    pub seen: BTreeMap<String, u32>,
    /// 보낸 만료 알림 키 `<grantId>:<72|24>`
    pub sent: BTreeSet<String>,
    /// 마지막 성공 조회의 리셋권 (조회 실패 중 만료 알림에 사용)
    pub cache: Vec<Grant>,
    pub last_ok: Option<DateTime<Utc>>,
    pub initialized: bool,
    /// 인증 실패 알림을 보낸 KST 날짜 (YYYY-MM-DD)
    pub auth_fail_day: Option<String>,
    /// 자동 사용을 끝낸 리셋권 id (성공·사용 불가 모두)
    #[serde(default)]
    pub auto_used: BTreeSet<String>,
}
