//! 안티목표 가드: 리셋권을 사용(consume)하는 요청이 코드에 없어야 한다.

use std::path::Path;

fn scan(dir: &Path, exts: &[&str], hits: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan(&p, exts, hits);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.contains(&x)) {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            for bad in ["consume", "reset_rate_limits", "rateLimitResetCredit/"] {
                if text.contains(bad) {
                    hits.push(format!("{}: {bad}", p.display()));
                }
            }
        }
    }
}

#[test]
fn no_reset_consume_calls_in_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    scan(&root.join("src"), &["rs"], &mut hits);
    scan(&root.join("../src"), &["ts", "js", "html"], &mut hits);
    assert!(hits.is_empty(), "consume 관련 코드 발견: {hits:?}");
}
