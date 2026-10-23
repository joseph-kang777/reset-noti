//! 알림 규칙 (스펙 6절). 순수 함수 — 시각은 인자로 받는다.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};

use crate::kst::fmt_kst;
pub use crate::model::Note;
use crate::model::{AccountState, Grant, Service};

const THRESHOLDS_H: [i64; 2] = [72, 24];
const STALE_AFTER_H: i64 = 2;

pub fn total(grants: &[Grant]) -> u32 {
    grants.iter().map(|g| g.count).sum()
}

fn label_of(svc: Service, label: &str) -> String {
    format!("{}({})", svc.display(), label)
}

/// 지금 해당하는 임계값들 (가까운 순서 아님, THRESHOLDS_H 순서)
fn due(g: &Grant, now: DateTime<Utc>) -> Vec<i64> {
    let Some(exp) = g.expires_at else { return vec![] };
    let left = exp - now;
    if left <= Duration::zero() {
        return vec![];
    }
    THRESHOLDS_H.iter().copied().filter(|h| left <= Duration::hours(*h)).collect()
}

fn key(id: &str, h: i64) -> String {
    format!("{id}:{h}")
}

/// 조회 성공 시 호출. 캐시·last_ok를 갱신하고 보낼 알림을 돌려준다.
pub fn on_fetch(st: &mut AccountState, svc: Service, label: &str, grants: &[Grant], now: DateTime<Utc>) -> Vec<Note> {
    let name = label_of(svc, label);
    let mut notes = Vec::new();
    let fresh: Vec<&Grant> = grants.iter().filter(|g| st.seen.get(&g.id).map_or(true, |c| g.count > *c)).collect();
    let n = total(grants);

    if !st.initialized {
        if !grants.is_empty() {
            let body = match grants.iter().filter_map(|g| g.expires_at).min() {
                Some(e) => format!("가장 빠른 만료: {}", fmt_kst(e)),
                None => "만료 시각 정보 없음".into(),
            };
            notes.push(Note { title: format!("{name} 리셋권 {n}개 보유 중"), body });
        }
    } else {
        for g in &fresh {
            let until = g.expires_at.map(|e| format!(" · {}까지", fmt_kst(e))).unwrap_or_default();
            notes.push(Note { title: format!("{name} 리셋권이 생겼어요"), body: format!("{}{until}\n현재 {n}개 보유", g.title) });
        }
    }

    // 이번에 처음 본(또는 처음 확인한) 리셋권의 이미 지난 임계값은 보낸 것으로 처리
    let suppress: Vec<&Grant> = if st.initialized { fresh } else { grants.iter().collect() };
    for g in suppress {
        for h in due(g, now) {
            st.sent.insert(key(&g.id, h));
        }
    }

    st.initialized = true;
    for g in grants {
        st.seen.insert(g.id.clone(), g.count);
    }
    st.cache = grants.to_vec();
    st.last_ok = Some(now);
    notes.extend(expiry_notes(st, svc, label, now));
    notes
}

/// 캐시 기준 만료 알림. 해당 임계값 중 가장 임박한 것 하나만 보내고 나머지는 보낸 것으로 처리.
pub fn expiry_notes(st: &mut AccountState, svc: Service, label: &str, now: DateTime<Utc>) -> Vec<Note> {
    let name = label_of(svc, label);
    let stale = st.last_ok.filter(|ok| now - *ok > Duration::hours(STALE_AFTER_H));
    let mut notes = Vec::new();
    let mut sent: BTreeSet<String> = std::mem::take(&mut st.sent);
    for g in &st.cache {
        let hs = due(g, now);
        let Some(&nearest) = hs.iter().min() else { continue };
        if !sent.contains(&key(&g.id, nearest)) {
            let mut body = format!("{} 만료", fmt_kst(g.expires_at.unwrap()));
            if nearest == 24 {
                body.push_str(" · 안 쓰면 사라져요");
            }
            if let Some(ok) = stale {
                body.push_str(&format!(" (마지막 확인: {})", fmt_kst(ok)));
            }
            notes.push(Note { title: format!("{name} 리셋권 {nearest}시간 남음"), body });
        }
        for h in hs {
            sent.insert(key(&g.id, h));
        }
    }
    st.sent = sent;
    notes
}

/// 아직 안 보낸 만료 알림 중 가장 이른 시각 (now 이후)
pub fn next_alert_at(st: &AccountState, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    st.cache
        .iter()
        .filter_map(|g| g.expires_at.map(|e| (g, e)))
        .flat_map(|(g, e)| THRESHOLDS_H.iter().map(move |h| (key(&g.id, *h), e - Duration::hours(*h))))
        .filter(|(k, at)| *at > now && !st.sent.contains(k))
        .map(|(_, at)| at)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kst::fmt_kst;
    use crate::model::{AccountState, Grant, Service};
    use chrono::{DateTime, Duration, TimeZone, Utc};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 23, 1, 0, 0).unwrap()
    }

    fn g(id: &str, hours_left: i64) -> Grant {
        gc(id, hours_left, 1)
    }

    fn gc(id: &str, hours_left: i64, count: u32) -> Grant {
        Grant { id: id.into(), title: "Full reset".into(), expires_at: Some(now() + Duration::hours(hours_left)), count }
    }

    fn fetch(st: &mut AccountState, grants: &[Grant], at: DateTime<Utc>) -> Vec<Note> {
        on_fetch(st, Service::Codex, "회사", grants, at)
    }

    fn titles(notes: &[Note]) -> Vec<String> {
        notes.iter().map(|n| n.title.clone()).collect()
    }

    #[test]
    fn first_run_sends_one_summary_and_marks_seen() {
        let mut st = AccountState::default();
        let notes = fetch(&mut st, &[g("a", 150), g("b", 300)], now());
        assert_eq!(titles(&notes), ["Codex(회사) 리셋권 2개 보유 중"]);
        assert!(notes[0].body.contains("10/29(목) 16:00"), "{}", notes[0].body);
        assert!(fetch(&mut st, &[g("a", 150), g("b", 300)], now()).is_empty());
    }

    #[test]
    fn first_run_with_no_grants_is_silent() {
        let mut st = AccountState::default();
        assert!(fetch(&mut st, &[], now()).is_empty());
        assert_eq!(titles(&fetch(&mut st, &[g("x", 200)], now())), ["Codex(회사) 리셋권이 생겼어요"]);
    }

    #[test]
    fn new_grant_notifies_once() {
        let mut st = AccountState::default();
        fetch(&mut st, &[g("a", 150)], now());
        let notes = fetch(&mut st, &[g("a", 150), g("b", 300)], now());
        assert_eq!(titles(&notes), ["Codex(회사) 리셋권이 생겼어요"]);
        assert!(notes[0].body.contains("현재 2개 보유"), "{}", notes[0].body);
        assert!(fetch(&mut st, &[g("a", 150), g("b", 300)], now()).is_empty());
    }

    #[test]
    fn expiry_alerts_at_72h_then_24h_once_each() {
        let mut st = AccountState::default();
        let gs = [g("a", 100)];
        fetch(&mut st, &gs, now());
        assert!(fetch(&mut st, &gs, now() + Duration::hours(27)).is_empty());
        assert_eq!(titles(&fetch(&mut st, &gs, now() + Duration::hours(28))), ["Codex(회사) 리셋권 72시간 남음"]);
        assert!(fetch(&mut st, &gs, now() + Duration::hours(29)).is_empty());
        assert_eq!(titles(&fetch(&mut st, &gs, now() + Duration::hours(76))), ["Codex(회사) 리셋권 24시간 남음"]);
        assert!(fetch(&mut st, &gs, now() + Duration::hours(80)).is_empty());
    }

    #[test]
    fn missed_72h_window_only_sends_24h() {
        let mut st = AccountState::default();
        let gs = [g("a", 100)];
        fetch(&mut st, &gs, now());
        assert_eq!(titles(&fetch(&mut st, &gs, now() + Duration::hours(80))), ["Codex(회사) 리셋권 24시간 남음"]);
        assert!(fetch(&mut st, &gs, now() + Duration::hours(81)).is_empty());
    }

    #[test]
    fn new_grant_already_near_expiry_does_not_double_alert() {
        let mut st = AccountState::default();
        fetch(&mut st, &[], now());
        assert_eq!(titles(&fetch(&mut st, &[g("a", 20)], now())), ["Codex(회사) 리셋권이 생겼어요"]);
        assert!(fetch(&mut st, &[g("a", 20)], now() + Duration::hours(1)).is_empty());
    }

    #[test]
    fn grant_without_expiry_never_gets_expiry_alert() {
        let mut st = AccountState::default();
        fetch(&mut st, &[], now());
        let grant = Grant { id: "x".into(), title: "리셋".into(), expires_at: None, count: 1 };
        assert_eq!(titles(&fetch(&mut st, &[grant.clone()], now())), ["Codex(회사) 리셋권이 생겼어요"]);
        assert!(fetch(&mut st, &[grant], now() + Duration::days(30)).is_empty());
    }

    #[test]
    fn fmt_kst_formats_weekday_and_time() {
        assert_eq!(fmt_kst(Utc.with_ymd_and_hms(2026, 10, 29, 19, 13, 0).unwrap()), "10/30(금) 04:13");
    }

    #[test]
    fn count_increase_on_same_id_is_new_grant() {
        let mut st = AccountState::default();
        fetch(&mut st, &[gc("a", 200, 1)], now());
        let notes = fetch(&mut st, &[gc("a", 200, 2)], now());
        assert_eq!(titles(&notes), ["Codex(회사) 리셋권이 생겼어요"]);
        assert!(notes[0].body.contains("현재 2개 보유"), "{}", notes[0].body);
    }

    #[test]
    fn expiry_from_cache_with_stale_suffix() {
        let mut st = AccountState::default();
        fetch(&mut st, &[g("a", 100)], now());
        let notes = expiry_notes(&mut st, Service::Codex, "회사", now() + Duration::hours(80));
        assert_eq!(titles(&notes), ["Codex(회사) 리셋권 24시간 남음"]);
        assert_eq!(notes[0].body, "10/27(화) 14:00 만료 · 안 쓰면 사라져요 (마지막 확인: 10/23(금) 10:00)");
    }

    #[test]
    fn no_stale_suffix_within_2h() {
        let mut st = AccountState::default();
        fetch(&mut st, &[g("a", 100)], now());
        let notes = fetch(&mut st, &[g("a", 100)], now() + Duration::hours(79));
        assert!(expiry_notes(&mut st, Service::Codex, "회사", now() + Duration::hours(80)).is_empty());
        assert_eq!(notes[0].body, "10/27(화) 14:00 만료 · 안 쓰면 사라져요");
    }

    #[test]
    fn stale_check_uses_last_ok_one_hour_ago() {
        let mut st = AccountState::default();
        fetch(&mut st, &[g("a", 100)], now() + Duration::hours(27));
        let notes = expiry_notes(&mut st, Service::Codex, "회사", now() + Duration::hours(28));
        assert_eq!(notes[0].body, "10/27(화) 14:00 만료");
    }

    #[test]
    fn next_alert_at_points_to_72h_mark() {
        let mut st = AccountState::default();
        fetch(&mut st, &[g("a", 100)], now());
        assert_eq!(next_alert_at(&st, now()), Some(now() + Duration::hours(28)));
        expiry_notes(&mut st, Service::Codex, "회사", now() + Duration::hours(28));
        assert_eq!(next_alert_at(&st, now() + Duration::hours(28)), Some(now() + Duration::hours(76)));
        expiry_notes(&mut st, Service::Codex, "회사", now() + Duration::hours(76));
        assert_eq!(next_alert_at(&st, now() + Duration::hours(76)), None);
    }

    #[test]
    fn total_sums_counts() {
        assert_eq!(total(&[gc("a", 1, 2), gc("b", 1, 1)]), 3);
    }
}
