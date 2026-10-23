//! Windows 토스트 알림 (B 스타일: 로고 + [사용량 페이지 열기] [닫기]).

use std::path::{Path, PathBuf};

use crate::model::{Note, Service};

pub const APP_NAME: &str = "리셋권 알림";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn file_uri(p: &Path) -> String {
    format!(
        "file:///{}",
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    )
}

pub fn toast_xml(svc: Service, note: &Note, logo: &Path) -> String {
    let url = esc(svc.usage_url());
    let lines: String = std::iter::once(note.title.as_str())
        .chain(note.body.split('\n'))
        .map(|t| format!("<text>{}</text>", esc(t)))
        .collect();
    format!(
        r#"<toast activationType="protocol" launch="{url}"><visual><binding template="ToastGeneric">{lines}<image placement="appLogoOverride" src="{logo}"/></binding></visual><actions><action content="사용량 페이지 열기" activationType="protocol" arguments="{url}"/><action content="닫기" activationType="system" arguments="dismiss"/></actions></toast>"#,
        logo = esc(&file_uri(logo)),
    )
}

#[derive(Debug, Clone)]
pub struct Notifier {
    app_id: String,
    icon_dir: PathBuf,
}

impl Notifier {
    pub fn new(app_id: &str, icon_dir: PathBuf) -> Self {
        Notifier {
            app_id: app_id.into(),
            icon_dir,
        }
    }

    fn logo(&self, svc: Service) -> PathBuf {
        self.icon_dir.join(match svc {
            Service::Claude => "claude.png",
            Service::Codex => "codex.png",
        })
    }

    /// 알림 표시 이름·아이콘 등록 (설치 여부와 무관하게 "리셋권 알림"으로 보이게)
    #[cfg(windows)]
    pub fn register_app(&self) -> Result<(), String> {
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(format!(r"Software\Classes\AppUserModelId\{}", self.app_id))
            .map_err(|e| e.to_string())?;
        key.set_value("DisplayName", &APP_NAME)
            .map_err(|e| e.to_string())?;
        key.set_value(
            "IconUri",
            &self.logo(Service::Codex).to_string_lossy().to_string(),
        )
        .map_err(|e| e.to_string())
    }

    #[cfg(windows)]
    pub fn toast(&self, svc: Service, note: &Note) -> Result<(), String> {
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
        use windows::core::HSTRING;

        let show = || -> windows::core::Result<()> {
            let doc = XmlDocument::new()?;
            doc.LoadXml(&HSTRING::from(toast_xml(svc, note, &self.logo(svc))))?;
            let toast = ToastNotification::CreateToastNotification(&doc)?;
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(&self.app_id))?
                .Show(&toast)
        };
        show().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Note, Service};
    use std::path::Path;

    #[test]
    fn xml_has_title_lines_logo_and_buttons() {
        let note = Note {
            title: "Codex(회사) 리셋권 24시간 남음".into(),
            body: "10/30(금) 04:13 만료\n현재 2개 <보유> & 끝".into(),
        };
        let xml = toast_xml(Service::Codex, &note, Path::new("C:/x/codex.png"));
        assert!(xml.contains("<text>Codex(회사) 리셋권 24시간 남음</text><text>10/30(금) 04:13 만료</text><text>현재 2개 &lt;보유&gt; &amp; 끝</text>"), "{xml}");
        assert!(
            xml.contains(r#"launch="https://chatgpt.com/codex/settings/usage""#),
            "{xml}"
        );
        assert!(
            xml.contains(r#"placement="appLogoOverride" src="file:///C:/x/codex.png""#),
            "{xml}"
        );
        assert!(xml.contains(r#"<action content="사용량 페이지 열기" activationType="protocol" arguments="https://chatgpt.com/codex/settings/usage"/>"#), "{xml}");
        assert!(
            xml.contains(r#"<action content="닫기" activationType="system" arguments="dismiss"/>"#),
            "{xml}"
        );
    }

    #[test]
    fn claude_uses_claude_url() {
        let note = Note {
            title: "t".into(),
            body: "b".into(),
        };
        assert!(
            toast_xml(Service::Claude, &note, Path::new("C:/c.png"))
                .contains("https://claude.ai/settings/usage")
        );
    }

    /// 화면에 알림 2개를 띄운다: cargo test toast_smoke -- --ignored
    #[test]
    #[ignore]
    fn toast_smoke() {
        let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
        let n = Notifier::new("io.github.joseph-kang777.resetnoti", icons);
        n.register_app().unwrap();
        n.toast(
            Service::Codex,
            &Note {
                title: "Codex(회사) 리셋권 24시간 남음".into(),
                body: "10/30(금) 04:13 만료 · 안 쓰면 사라져요".into(),
            },
        )
        .unwrap();
        n.toast(
            Service::Claude,
            &Note {
                title: "Claude(개인) 리셋권이 생겼어요".into(),
                body: "weekly reset · 11/1(일) 09:00까지\n현재 1개 보유".into(),
            },
        )
        .unwrap();
    }
}
