# 리셋권 알림 데스크톱 앱 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 여러 Claude/Codex 계정의 리셋권을 감시해서, 새로 생기거나 만료가 다가오면(72h/24h 전) Windows 알림을 보내는 트레이 상주 Tauri 앱을 만든다.

**Architecture:** Rust 백엔드가 계정별 전용 CLI 설정 디렉터리(`CLAUDE_CONFIG_DIR` / `CODEX_HOME`)를 관리하고, 공식 CLI로 로그인·토큰 갱신을 맡긴다. Codex는 `codex app-server` JSON-RPC로, Claude는 `/api/oauth/usage?cedar_ember=1`로 리셋권을 조회한다. 순수 함수인 알림 규칙(`rules`)이 계정별 상태를 갱신하고, 스케줄러가 1시간 폴링 + 리셋권별 정각 타이머로 알림을 띄운다. 화면은 Vite + 바닐라 TS.

**Tech Stack:** Tauri 2, Rust (tokio, serde, chrono, reqwest(rustls), uuid, which, tauri-winrt-notification), tauri-plugin-autostart, tauri-plugin-updater, tauri-plugin-dialog, tauri-plugin-opener, Vite + TypeScript, pnpm, GitHub Actions (tauri-action).

**Spec:** `docs/superpowers/specs/2026-10-23-reset-noti-desktop-design.md`

## Global Constraints

- 리셋권 **consume 요청 금지**. `account/rateLimitResetCredit/consume`, `/wham/rate-limit-reset-credits/consume`, `/reset_rate_limits` 문자열이 코드에 존재하면 안 된다(Task 3에 grep 테스트).
- 토큰은 로그·UI·알림·에러 메시지에 절대 포함하지 않는다.
- 사용자 기존 `~/.claude`, `~/.codex`를 읽거나 쓰지 않는다. 모든 CLI 실행은 계정 디렉터리 환경변수를 **반드시** 설정한다.
- 앱이 Claude Code client_id로 OAuth/토큰 갱신을 직접 하지 않는다.
- 시각 표시는 항상 KST, 형식 `M/D(요일) HH:MM` (예: `10/30(금) 04:13`), 요일 `월화수목금토일`. 시스템 시간대와 무관.
- 알림 임계값 `[72, 24]` 시간. 알림 앱 이름 `리셋권 알림`. 앱 identifier `io.github.joseph-kang777.resetnoti` (내 판단), 제품명 `리셋권 알림`.
- 데이터 위치 `%APPDATA%\ResetNoti\` (`accounts\{claude|codex}-<uuid>\`, `accounts.json`, `state.json`, `reset_noti.log`).
- 사용량 페이지: Claude `https://claude.ai/settings/usage`, Codex `https://chatgpt.com/codex/settings/usage`(Task 9에서 확인).
- 알림 문구(정확히):
  - 첫 확인 요약: 제목 `{서비스}({라벨}) 리셋권 {N}개 보유 중`, 본문 `가장 빠른 만료: {시각}` 또는 `만료 시각 정보 없음`
  - 새 리셋권: 제목 `{서비스}({라벨}) 리셋권이 생겼어요`, 본문 `{title} · {시각}까지` (만료 없으면 `{title}`) + 줄바꿈 + `현재 {N}개 보유`
  - 만료: 제목 `{서비스}({라벨}) 리셋권 {72|24}시간 남음`, 본문 `{시각} 만료` + 24h일 때 ` · 안 쓰면 사라져요` + stale일 때 ` (마지막 확인: {시각})`
  - 인증 실패: 제목 `{서비스}({라벨}) 재인증 필요`, 본문 `앱에서 [재인증]을 눌러 다시 로그인해 주세요.` (계정당 KST 하루 1회)
- 계정 줄: `{email} · 리셋권 {N}개 · {가장 빠른 만료} 만료`, 리셋권 0개면 `{email} · 리셋권 없음`, 상태 배지 `재인증 필요` / `확인 실패`.

## Review Focus

1. **CLI가 PATH에 없거나 npm shim(`codex.cmd`, `claude.exe`)인 경우** — `Command::new("codex")`는 Windows에서 `.cmd`를 못 찾는다. `which` 크레이트로 해석하고, 없으면 섹션에 `Codex CLI가 필요합니다` 안내. → Task 4 `resolve_cli` 테스트.
2. **사용자가 브라우저 로그인을 중간에 닫음** — 로그인 프로세스가 영원히 대기. 10분 타임아웃 + [취소] 시 프로세스 종료, 계정 디렉터리 삭제, 목록에 남지 않음. → Task 7 테스트.
3. **절전 중 72h·24h 시점을 모두 지나침** — 깨어나서 24h 알림 하나만. → Task 2 `missed_72h_window_only_sends_24h`, Task 8 wake 테스트.
4. **app-server / claude 명령이 멈춤** — 30초 타임아웃 후 kill, 해당 계정 `확인 실패`, 다른 계정 확인은 계속. → Task 4/5 타임아웃 테스트.
5. **수동 [지금 확인]과 정기 확인이 동시에 실행** — 같은 알림 중복 발송. 확인은 전역 `tokio::sync::Mutex`로 직렬화. → Task 8 테스트.

---

### Task 0: 저장소 정리 + Tauri 골격

**Files:**
- Move: `reset_noti.py`, `test_reset_noti.py`, `install.ps1`, `uninstall.ps1`, `assets/` → `prototype/`
- Create: `app/` (Tauri 2, vanilla-ts 템플릿), `README.md`, 루트 `.gitignore`에 `app/node_modules/`, `app/src-tauri/target/`, `app/dist/` 추가

- [ ] **Step 1:** `git init`, 위 파일 이동(`.superpowers/`는 이미 ignore).
- [ ] **Step 2:** `pnpm create tauri-app app --template vanilla-ts --manager pnpm --yes` (identifier `io.github.joseph-kang777.resetnoti`, productName `리셋권 알림`). `prototype/assets/*.png`를 `app/src-tauri/icons/claude.png`, `codex.png`로 복사하고 `pnpm tauri icon prototype/assets/codex.png`로 앱 아이콘 생성.
- [ ] **Step 3:** `cd app && pnpm install && pnpm tauri build --debug --no-bundle` → 성공.
- [ ] **Step 4:** Commit `chore: scaffold tauri app, move python prototype`.

### Task 1: 위험 검증 스파이크 (사용자 참여 필요, 코드 버림)

결과를 스펙 5절/7절에 기록하는 것이 산출물. 스크래치 디렉터리에서 진행.

- [ ] **Step 1 (Claude 갱신):** 사용자에게 `$env:CLAUDE_CONFIG_DIR="<scratch>\c1"; claude auth login` 실행 요청 → 로그인. `.credentials.json`을 사본으로 두고 `claudeAiOauth.expiresAt`을 과거(ms)로 바꾼 뒤 `claude auth status --json` 실행. 판정: `accessToken`이 바뀌고 `expiresAt`이 미래면 PASS(A경로). 실패 시 `claude -p "ok" --model haiku --max-turns 1`로 같은 검사(B경로). 둘 다 실패면 사용자에게 보고하고 중단.
  - 토큰 값은 출력하지 말고 "바뀜/안 바뀜"과 `expiresAt`만 출력하는 스크립트로 확인.
- [ ] **Step 2 (GUI에서 로그인):** 콘솔 없이(`CREATE_NO_WINDOW`) `claude auth login`, `codex login`을 띄웠을 때 브라우저가 열리고 성공 후 프로세스가 종료되는지. stdin 입력을 요구하면 `CREATE_NEW_CONSOLE`(보이는 콘솔)로 대체 — 결과 기록.
- [ ] **Step 3 (Codex 계정 정보):** 새 `CODEX_HOME`으로 로그인 후 app-server `account/read` 응답 형태 기록(이메일 위치). 없으면 `auth.json` id_token JWT payload의 `email` 클레임과 `https://api.openai.com/auth`.`chatgpt_account_id`.
- [ ] **Step 4:** 결과를 스펙에 반영하고 Commit `docs: record spike results`.

### Task 2: 시간 형식 + 알림 규칙 (순수 로직)

**Files:**
- Create: `app/src-tauri/src/model.rs`, `app/src-tauri/src/kst.rs`, `app/src-tauri/src/rules.rs`

**Interfaces:**
- Produces:
  - `enum Service { Claude, Codex }` (serde `"claude"|"codex"`), `fn display(&self) -> &'static str` (`"Claude"|"Codex"`), `fn usage_url(&self) -> &'static str`
  - `struct Grant { id: String, title: String, expires_at: Option<DateTime<Utc>>, count: u32 }` (Serialize/Deserialize/Clone/PartialEq)
  - `struct Note { title: String, body: String }`
  - `struct AccountState { seen: BTreeMap<String,u32> /*id→count*/, sent: BTreeSet<String>, cache: Vec<Grant>, last_ok: Option<DateTime<Utc>>, initialized: bool, auth_fail_day: Option<String> }` (Default, serde)
  - `fn fmt_kst(dt: DateTime<Utc>) -> String`
  - `fn on_fetch(st: &mut AccountState, svc: Service, label: &str, grants: &[Grant], now: DateTime<Utc>) -> Vec<Note>` — 캐시·last_ok 갱신 + 요약/새 리셋권 알림 + 즉시 해당하는 만료 알림
  - `fn expiry_notes(st: &mut AccountState, svc: Service, label: &str, now: DateTime<Utc>) -> Vec<Note>` — `st.cache` 기준 만료 알림 (stale 접미사 포함)
  - `fn next_alert_at(st: &AccountState, now: DateTime<Utc>) -> Option<DateTime<Utc>>` — 아직 안 보낸 가장 이른 `expires_at - 72h|24h` (now 이후)
  - `fn total(grants: &[Grant]) -> u32` (count 합)

- [ ] **Step 1: 실패하는 테스트 작성** (`rules.rs` 내 `#[cfg(test)]`). `NOW = 2026-10-23T01:00Z`, `g(id, hours_left)` 헬퍼(count 1, title `Full reset`), label `회사`, svc Codex. 프로토타입 `prototype/test_reset_noti.py`의 7개 규칙 테스트를 같은 값으로 옮긴다:
  - `first_run_sends_one_summary_and_marks_seen`: g(a,150), g(b,300) → 제목 `["Codex(회사) 리셋권 2개 보유 중"]`, 본문에 `10/29(목) 16:00`; 재호출 `[]`
  - `first_run_with_no_grants_is_silent` → `[]`, 이후 g(x,200) → `Codex(회사) 리셋권이 생겼어요`
  - `new_grant_notifies_once` → 본문에 `현재 2개 보유`
  - `expiry_alerts_at_72h_then_24h_once_each` (g(a,100): +27h 없음, +28h `72시간 남음`, +29h 없음, +76h `24시간 남음`, +80h 없음) — `on_fetch` 경로
  - `missed_72h_window_only_sends_24h`
  - `new_grant_already_near_expiry_does_not_double_alert`
  - `grant_without_expiry_never_gets_expiry_alert`
  - 추가: `fmt_kst(2026-10-29T19:13Z) == "10/30(금) 04:13"`
  - 추가: `count_increase_on_same_id_is_new_grant` — 첫 fetch g(a,200,count 1), 다음 count 2 → `리셋권이 생겼어요`, 본문 `현재 2개 보유`
  - 추가: `expiry_from_cache_with_stale_suffix` — on_fetch(NOW) 후 `expiry_notes(now=NOW+80h)` (g(a,100)) → 본문 `10/27(화) 14:00 만료 · 안 쓰면 사라져요 (마지막 확인: 10/23(금) 10:00)`
  - 추가: `no_stale_suffix_within_2h` — last_ok가 now-1h면 접미사 없음
  - 추가: `next_alert_at_points_to_72h_mark` — g(a,100) 직후 → `NOW+28h`; 72 보낸 뒤 → `NOW+76h`; 모두 보내면 `None`
- [ ] **Step 2:** `cargo test --manifest-path app/src-tauri/Cargo.toml rules::` → 컴파일 실패 확인.
- [ ] **Step 3:** 구현. 규칙은 스펙 6절 1~5, 8, 9와 프로토타입 `plan()`과 동일. `on_fetch`는 마지막에 `expiry_notes`를 호출하되 이번에 새로 본 id의 해당 키는 미리 `sent`에 넣는다. stale = `now - last_ok > 2h`. 만료된 리셋권은 무시.
- [ ] **Step 4:** 테스트 PASS.
- [ ] **Step 5:** Commit `feat: alert rules with KST formatting`.

### Task 3: 저장소(store) + consume 금지 가드

**Files:**
- Create: `app/src-tauri/src/store.rs`, `app/src-tauri/tests/no_consume.rs`

**Interfaces:**
- Consumes: `Service`, `AccountState` (Task 2)
- Produces:
  - `struct Account { id: Uuid, service: Service, alias: Option<String>, email: Option<String>, identity: Option<String> /*Claude orgId+email, Codex account_id*/, added_at: DateTime<Utc> }`, `fn label(&self) -> String` (alias → email → `"계정"`)
  - `struct Store { root: PathBuf }`, `fn open(root: PathBuf) -> Store`, `fn default_root() -> PathBuf` (`%APPDATA%\ResetNoti`)
  - `fn account_dir(&self, a: &Account) -> PathBuf` (`accounts\{service}-{uuid}`)
  - `fn load_accounts(&self) -> Vec<Account>`, `fn save_accounts(&self, &[Account]) -> io::Result<()>`
  - `fn load_state(&self) -> HashMap<Uuid, AccountState>`, `fn save_state(&self, &HashMap<Uuid, AccountState>) -> io::Result<()>`
- 파일 쓰기는 임시 파일 + rename(원자적). 손상된 JSON은 `.bak`로 옮기고 빈 값으로 시작(로그).

- [ ] **Step 1:** 테스트: tempdir에서 저장→로드 왕복, 손상 JSON → 빈 Vec + `.bak` 생성, `label()` 우선순위.
- [ ] **Step 2:** `no_consume.rs`: `app/src-tauri/src/**/*.rs`와 `app/src/**/*.ts`를 읽어 `consume`, `reset_rate_limits` 문자열이 없음을 assert.
- [ ] **Step 3:** 구현 → `cargo test` PASS.
- [ ] **Step 4:** Commit `feat: account/state store`.

### Task 4: Codex 조회 (app-server)

**Files:**
- Create: `app/src-tauri/src/cli.rs`, `app/src-tauri/src/codex.rs`, `app/src-tauri/tests/fixtures/codex_rate_limits.json` (스펙 5절/프로토타입에서 확인한 실제 응답 형태: `rateLimitResetCredits.credits[]` with `expiresAt` unix 초)

**Interfaces:**
- Produces:
  - `cli.rs`: `fn resolve_cli(name: &str) -> Option<PathBuf>` (`which`), `async fn run_json(cmd: Command, timeout: Duration) -> Result<Value, CliError>`, `enum CliError { NotInstalled, Timeout, Exit(i32, String /*stderr 앞 500자, 토큰 마스킹*/), Parse }`. 모든 자식은 `CREATE_NO_WINDOW`, `kill_on_drop(true)`.
  - `codex.rs`: `struct Fetched { grants: Vec<Grant>, email: Option<String>, identity: Option<String> }`, `enum FetchError { NotInstalled, Auth, Other(String) }`, `async fn fetch(dir: &Path) -> Result<Fetched, FetchError>`, `fn parse_rate_limits(v: &Value) -> Vec<Grant>` (`status=="available"`만, `count=1`, `title` 없으면 `리셋`)
  - JSON-RPC: `initialize`(clientInfo `{name:"reset-noti",version}`) → `initialized` 알림 → `account/rateLimits/read` → (Task 1 결과대로) 계정 정보. 응답의 인증 오류(에러 메시지에 `auth`/`login`/401)는 `FetchError::Auth`. 전체 30초 타임아웃.

- [ ] **Step 1:** 테스트: fixture 파싱(available 2개, redeemed 제외, `expiresAt 1793301211` → `2026-10-29T19:13:31Z`), `resolve_cli("definitely-not-a-cli") == None`, `run_json`가 `ping -n 60 127.0.0.1` 같은 장기 명령에 1초 타임아웃 → `CliError::Timeout`.
- [ ] **Step 2:** 실패 확인 → 구현 → PASS.
- [ ] **Step 3 (수동):** `CODEX_HOME`=Task 1 스크래치 계정으로 `cargo test -- --ignored codex_live` (ignored 테스트) → 리셋권 목록 출력.
- [ ] **Step 4:** Commit `feat: codex reset credits via app-server`.

### Task 5: Claude 조회 + 토큰 갱신

**Files:**
- Create: `app/src-tauri/src/claude.rs`, `app/src-tauri/tests/fixtures/claude_usage_empty.json` (프로토타입 응답의 `cedar_ember` 블록), `claude_usage_grants.json` (가정 형태: `grants:[{"grant_id":"g1","kind":"weekly","resets_left":2,"expires_at":"2026-11-01T00:00:00+00:00"},"g2"]`)

**Interfaces:**
- Consumes: `cli::{resolve_cli, run_json, CliError}`, `codex::{Fetched, FetchError}` (같은 타입 재사용 — `fetch.rs`로 옮겨도 됨)
- Produces: `async fn fetch(dir: &Path) -> Result<Fetched, FetchError>`, `fn parse_usage(v: &Value) -> Vec<Grant>`, `async fn ensure_fresh(dir: &Path) -> Result<(), FetchError>`, `async fn whoami(dir: &Path) -> Result<(String /*email*/, String /*orgId*/), FetchError>` (`claude auth status --json`)
- `ensure_fresh`: `<dir>/.credentials.json`의 `expiresAt`(ms)가 now+10분 이후면 OK. 아니면 Task 1에서 PASS한 경로(A: `auth status`, B: `claude -p "ok" --model haiku --max-turns 1`)를 `CLAUDE_CONFIG_DIR=dir`로 실행(60초 타임아웃) 후 재확인. 여전히 만료면 `Auth`.
- HTTP: `GET https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1`, 헤더 `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20`, `User-Agent: reset-noti/<ver>`; 401/403 → `Auth`. `grants` 비어있지 않으면 `cedar_ember` 원문을 로그(토큰 없음).
- 파싱 규칙: 스펙 5절 + 9항 (`resets_left` 있으면 count, 없으면 1; 문자열 원소 허용; title = `{kind} reset` 또는 `리셋`).

- [ ] **Step 1:** 테스트: empty fixture → `[]`; grants fixture → `[(g1, "weekly reset", 2, Some(2026-11-01)), (g2, "리셋", 1, None)]`; `cedar_ember: null` → `[]`; `ensure_fresh`가 미래 expiresAt인 임시 credentials에 대해 명령을 실행하지 않고 Ok(명령 경로를 존재하지 않는 CLI로 주입해 검증).
- [ ] **Step 2:** 실패 확인 → 구현 → PASS.
- [ ] **Step 3 (수동):** Task 1 스크래치 Claude 계정으로 ignored `claude_live` 테스트.
- [ ] **Step 4:** Commit `feat: claude reset grants with token refresh via CLI`.

### Task 6: Windows 알림

**Files:**
- Create: `app/src-tauri/src/notify.rs`

**Interfaces:**
- Consumes: `Service`, `Note`
- Produces: `fn toast(svc: Service, note: &Note) -> Result<(), String>`
- `tauri-winrt-notification`(Toast) 사용, app id = 번들 identifier (설치본) / 개발 중엔 `Toast::POWERSHELL_APP_ID` 폴백. 로고 = 리소스의 `icons/{claude|codex}.png`(`IconCrop::Square`), 클릭 launch = `usage_url`, 버튼 `사용량 페이지 열기`(protocol, usage_url) / `닫기`(dismiss). 크레이트가 버튼을 지원하지 않으면 XML을 직접 만들어 `windows` 크레이트 `ToastNotificationManager`로 표시(프로토타입 `show_toast`의 XML과 동일 구조).
- [ ] **Step 1:** ignored 테스트 `toast_smoke`로 B 스타일 두 개 표시 → 사용자 눈으로 확인(스펙 3절 예시 문구).
- [ ] **Step 2:** Commit `feat: windows toast with usage button`.

### Task 7: 계정 추가/재인증/제거

**Files:**
- Create: `app/src-tauri/src/accounts.rs`

**Interfaces:**
- Consumes: `Store`, `Account`, `cli`, `claude::whoami`, `codex::fetch`(계정 정보)
- Produces:
  - `async fn add(store: &Store, svc: Service, cancel: CancellationToken) -> Result<Account, AddError>`
  - `async fn reauth(store: &Store, id: Uuid, cancel: CancellationToken) -> Result<(), AddError>`
  - `fn remove(store: &Store, id: Uuid) -> io::Result<()>` (디렉터리 + 목록 + state 제거)
  - `fn rename(store: &Store, id: Uuid, alias: Option<String>) -> io::Result<()>`
  - `enum AddError { NotInstalled, Cancelled, Timeout, Duplicate(String /*기존 라벨*/), Failed(String) }`
- 흐름: 새 uuid 디렉터리 생성 → 로그인 명령(Task 1 Step 2에서 정한 창 모드, env `CLAUDE_CONFIG_DIR`/`CODEX_HOME`) → 10분 타임아웃/취소 시 kill + 디렉터리 삭제 → 성공 시 신원 조회 → `identity` 중복이면 디렉터리 삭제 후 `Duplicate` → 목록 저장. 재인증은 같은 디렉터리로 로그인만 재실행하고 state의 `auth_fail_day` 초기화.
- [ ] **Step 1:** 테스트(로그인 명령을 주입 가능한 `LoginRunner` trait로 분리): 취소 → 디렉터리 없음 & 목록 불변; 타임아웃 → 동일; 중복 identity → `Duplicate("회사")` & 디렉터리 없음; 성공 → 목록 1개, email 채워짐; remove → 디렉터리·state 삭제.
- [ ] **Step 2:** 실패 확인 → 구현 → PASS.
- [ ] **Step 3:** Commit `feat: add/reauth/remove accounts via official CLI login`.

### Task 8: 확인 루프 + 정각 타이머

**Files:**
- Create: `app/src-tauri/src/scheduler.rs`

**Interfaces:**
- Consumes: `Store`, `rules::{on_fetch, expiry_notes, next_alert_at}`, `claude::fetch`, `codex::fetch`, `notify::toast`
- Produces:
  - `struct Checker { store, lock: tokio::sync::Mutex<()>, fetcher: Box<dyn Fetcher>, notifier: Box<dyn Notifier>, clock: Box<dyn Clock> }`
  - `async fn check_all(&self) -> Vec<AccountView>` — 계정 순차 확인. Ok → `on_fetch`; Auth → 상태 `재인증 필요` + 하루 1회 인증 실패 알림 + `expiry_notes`(캐시); Other/Timeout → `확인 실패` + `expiry_notes`. 마지막에 state 저장.
  - `async fn fire_due(&self)` — 모든 계정 `expiry_notes(now)`.
  - `fn spawn(app: AppHandle, checker: Arc<Checker>)` — 1시간 interval로 `check_all`; 별도 루프에서 `min(next_alert_at)`까지(최대 5분 단위로 끊어) 대기 후 `fire_due`; 대기 중 벽시계가 예상보다 2분 이상 점프하면(절전 복귀) 즉시 `check_all`. 결과는 `accounts-changed` 이벤트로 UI에 emit.
  - `struct AccountView { id, service, label, email, count: u32, earliest: Option<String /*fmt_kst*/>, status: "ok"|"auth"|"error"|"checking", last_ok: Option<String> }`
- [ ] **Step 1:** 테스트(가짜 Fetcher/Notifier/Clock): 두 `check_all`을 동시에 실행해도 알림 1번; Auth 실패여도 캐시 기반 24h 알림 발송 + 인증 실패 알림 하루 1번; 한 계정 타임아웃이어도 다음 계정 확인됨; 시계를 +80h로 점프시킨 뒤 `fire_due` → 24h 알림 1개.
- [ ] **Step 2:** 실패 확인 → 구현 → PASS.
- [ ] **Step 3:** Commit `feat: polling loop and exact-time expiry timers`.

### Task 9: 화면 + 트레이 + 자동 시작

**Files:**
- Modify: `app/src-tauri/src/lib.rs` (플러그인·트레이·커맨드 등록), `app/src-tauri/tauri.conf.json`, `app/src-tauri/capabilities/default.json`
- Create: `app/src-tauri/src/commands.rs`, `app/src/main.ts`, `app/src/styles.css`, `app/index.html`

**Interfaces:**
- Consumes: `Checker`, `accounts::*`, `AccountView`
- Produces (Tauri 커맨드): `list_accounts() -> Vec<AccountView>`, `add_account(service) -> Result<AccountView, String>`, `cancel_add()`, `reauth(id)`, `remove_account(id)` (UI에서 dialog 확인 후), `rename_account(id, alias)`, `check_now() -> Vec<AccountView>`, `cli_status() -> {claude: bool, codex: bool}`, `get_autostart() -> bool`, `set_autostart(bool)`
- 화면: 다크 테마, Orca "AI 제공자 계정" 화면 구조. 상단 [지금 확인] + 마지막 확인 시각 + 자동 시작 토글. Claude/Codex 섹션 카드, 각 `+ 계정 추가`(CLI 없으면 `Claude Code CLI가 필요합니다` / `Codex CLI가 필요합니다`), 계정 줄은 Global Constraints 형식 + 별칭 인라인 편집 + `재인증` `제거` + 상태 배지. 추가 중엔 `브라우저에서 로그인해 주세요… [취소]`.
- 트레이: 좌클릭 창 열기, 메뉴 `열기` / `지금 확인` / `종료`. 창 닫기 = 숨김. 단일 인스턴스(`tauri-plugin-single-instance`).
- 자동 시작: `tauri-plugin-autostart`, 최초 실행 시 켬(`--minimized` 인자로 트레이만).
- [ ] **Step 1:** `pnpm tauri dev`로 실행, 수동 체크리스트: 계정 추가(두 서비스) → 줄 형식 확인 → 별칭 변경 → 알림 제목에 별칭 반영 → 재인증 → 제거 확인창 → 창 닫아도 트레이 유지 → Codex [사용량 페이지 열기] 주소가 실제 사용량 화면인지 확인(아니면 상수 수정).
- [ ] **Step 2:** `pnpm tsc --noEmit` 통과.
- [ ] **Step 3:** Commit `feat: settings window, tray, autostart`.

### Task 10: 자동 업데이트 + 릴리스 파이프라인 + README

**Files:**
- Modify: `app/src-tauri/tauri.conf.json` (`bundle.createUpdaterArtifacts: true`, `plugins.updater.endpoints: ["https://github.com/<owner>/<repo>/releases/latest/download/latest.json"]`, `pubkey`), `lib.rs`
- Create: `.github/workflows/release.yml`, `README.md`

- [ ] **Step 1:** `pnpm tauri signer generate -w %USERPROFILE%\.tauri\resetnoti.key` (비밀번호는 사용자 입력). 공개키만 conf에 넣는다. **비밀 키·비밀번호는 저장소에 넣지 않는다.**
- [ ] **Step 2:** 앱 시작 시 + 24시간마다 `updater().check()`; 새 버전이면 dialog `새 버전 {v}이(가) 있어요. 지금 설치할까요?` → 예 → 다운로드·설치·재시작.
- [ ] **Step 3:** `release.yml`: `v*` 태그 push → windows-latest, pnpm, `tauri-apps/tauri-action@v0`(NSIS), secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `includeUpdaterJson: true`, 릴리스 생성.
- [ ] **Step 4:** README(한국어): 필요 조건(Windows 10/11, 쓰는 서비스의 CLI), 설치(SmartScreen 경고 → 추가 정보 → 실행), 계정 추가 방법, 기존 Python 버전 제거(`prototype/uninstall.ps1`), 릴리스 방법(태그 push), 알려진 한계(비공식 API, Claude 리셋권 형태 미확인).
- [ ] **Step 5:** 사용자와 GitHub 저장소 생성(공개 여부는 사용자 결정) → secrets 등록 → `v0.1.0` 태그 → 릴리스 산출물(`*-setup.exe`, `latest.json`) 확인 → `v0.1.1`로 업데이트 동작 확인.
- [ ] **Step 6:** Commit `feat: auto-update and release pipeline`.
