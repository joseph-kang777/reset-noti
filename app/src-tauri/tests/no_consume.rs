//! 가드: 리셋권 사용 요청은 codex.rs(만료 1시간 전 자동 사용) 한 곳에만 있어야 한다.

use std::path::Path;

fn scan(dir: &Path, exts: &[&str], hits: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan(&p, exts, hits);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| exts.contains(&x))
            && p.file_name().is_none_or(|n| n != "codex.rs")
        {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            for bad in [
                "rateLimitResetCredit/",
                "reset_rate_limits",
                "rate-limit-reset-credits",
            ] {
                if text.contains(bad) {
                    hits.push(format!("{}: {bad}", p.display()));
                }
            }
        }
    }
}

#[test]
fn reset_consume_only_in_codex_rs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    scan(&root.join("src"), &["rs"], &mut hits);
    scan(&root.join("../src"), &["ts", "js", "html"], &mut hits);
    assert!(
        hits.is_empty(),
        "codex.rs 밖에서 리셋권 사용 코드 발견: {hits:?}"
    );
}
