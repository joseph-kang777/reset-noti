use chrono::{DateTime, Datelike, FixedOffset, Utc};

const WEEKDAYS: [&str; 7] = ["월", "화", "수", "목", "금", "토", "일"];

fn kst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).unwrap()
}

/// `10/30(금) 04:13` — 시스템 시간대와 무관하게 KST.
pub fn fmt_kst(dt: DateTime<Utc>) -> String {
    let k = dt.with_timezone(&kst());
    format!(
        "{}/{}({}) {}",
        k.month(),
        k.day(),
        WEEKDAYS[k.weekday().num_days_from_monday() as usize],
        k.format("%H:%M")
    )
}

/// KST 날짜 `YYYY-MM-DD` (하루 1회 알림 판정용)
pub fn kst_day(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&kst()).format("%Y-%m-%d").to_string()
}
