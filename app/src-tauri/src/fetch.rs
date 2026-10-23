use crate::model::Grant;

/// 계정 하나의 조회 결과
#[derive(Debug, Clone)]
pub struct Fetched {
    pub grants: Vec<Grant>,
    pub email: Option<String>,
    pub identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    NotInstalled,
    Auth,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::NotInstalled => write!(f, "CLI가 설치되어 있지 않음"),
            FetchError::Auth => write!(f, "인증 필요"),
            FetchError::Other(s) => write!(f, "{s}"),
        }
    }
}
