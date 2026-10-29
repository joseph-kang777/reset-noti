//! %APPDATA%\ResetNoti 아래 계정 목록·상태 저장.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{AccountState, Service};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
    pub service: Service,
    pub alias: Option<String>,
    pub email: Option<String>,
    /// 중복 판별용 (Claude: orgId+email, Codex: chatgptAccountId+email)
    pub identity: Option<String>,
    pub added_at: DateTime<Utc>,
    /// 만료 1시간 전 자동 사용 (Codex만, 기본 켜짐)
    #[serde(default = "default_true")]
    pub auto_use: bool,
}

fn default_true() -> bool {
    true
}

impl Account {
    pub fn label(&self) -> String {
        self.alias
            .clone()
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| "계정".into())
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn open(root: PathBuf) -> Store {
        Store { root }
    }

    pub fn default_root() -> PathBuf {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        appdata.join("ResetNoti")
    }

    pub fn account_dir(&self, a: &Account) -> PathBuf {
        let svc = match a.service {
            Service::Claude => "claude",
            Service::Codex => "codex",
        };
        self.root.join("accounts").join(format!("{svc}-{}", a.id))
    }

    pub fn load_accounts(&self) -> Vec<Account> {
        self.load("accounts.json")
    }

    pub fn save_accounts(&self, accounts: &[Account]) -> io::Result<()> {
        self.save("accounts.json", &accounts)
    }

    pub fn load_state(&self) -> HashMap<Uuid, AccountState> {
        self.load("state.json")
    }

    pub fn save_state(&self, state: &HashMap<Uuid, AccountState>) -> io::Result<()> {
        self.save("state.json", state)
    }

    fn load<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        let path = self.root.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return T::default();
        };
        match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("{name} 손상, .bak으로 이동: {e}");
                let _ = std::fs::rename(&path, bak(&path));
                T::default()
            }
        }
    }

    /// 임시 파일에 쓴 뒤 rename (원자적)
    fn save<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(name);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
        std::fs::rename(&tmp, &path)
    }
}

fn bak(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountState, Service};
    use chrono::Utc;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn acct(alias: Option<&str>, email: Option<&str>) -> Account {
        Account {
            id: Uuid::new_v4(),
            service: Service::Codex,
            alias: alias.map(Into::into),
            email: email.map(Into::into),
            identity: None,
            added_at: Utc::now(),
            auto_use: true,
        }
    }

    #[test]
    fn accounts_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let a = acct(Some("회사"), Some("a@b.c"));
        store.save_accounts(&[a.clone()]).unwrap();
        let back = store.load_accounts();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, a.id);
        assert_eq!(back[0].alias.as_deref(), Some("회사"));
    }

    #[test]
    fn state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let id = Uuid::new_v4();
        let mut st = AccountState::default();
        st.sent.insert("x:24".into());
        store.save_state(&HashMap::from([(id, st)])).unwrap();
        assert!(store.load_state()[&id].sent.contains("x:24"));
    }

    #[test]
    fn missing_files_load_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("nope"));
        assert!(store.load_accounts().is_empty());
        assert!(store.load_state().is_empty());
    }

    #[test]
    fn corrupt_json_moves_to_bak_and_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("accounts.json"), "{not json").unwrap();
        let store = Store::open(dir.path().to_path_buf());
        assert!(store.load_accounts().is_empty());
        assert!(dir.path().join("accounts.json.bak").exists());
        assert!(!dir.path().join("accounts.json").exists());
    }

    #[test]
    fn auto_use_defaults_on_for_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"[{"id":"00000000-0000-0000-0000-000000000001","service":"codex","alias":null,"email":null,"identity":null,"added_at":"2026-10-23T00:00:00Z"}]"#;
        std::fs::write(dir.path().join("accounts.json"), json).unwrap();
        assert!(Store::open(dir.path().to_path_buf()).load_accounts()[0].auto_use);
    }

    #[test]
    fn label_prefers_alias_then_email() {
        assert_eq!(acct(Some("회사"), Some("a@b.c")).label(), "회사");
        assert_eq!(acct(None, Some("a@b.c")).label(), "a@b.c");
        assert_eq!(acct(None, None).label(), "계정");
    }

    #[test]
    fn account_dir_is_service_and_uuid() {
        let store = Store::open(std::path::PathBuf::from("R"));
        let a = acct(None, None);
        assert_eq!(
            store.account_dir(&a),
            std::path::PathBuf::from("R")
                .join("accounts")
                .join(format!("codex-{}", a.id))
        );
    }
}
