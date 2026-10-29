# 리셋권 알림

Claude / Codex 계정의 **리셋권**(사용량 한도 즉시 초기화권)이 새로 생기거나 만료가 다가오면 Windows 알림으로 알려주는 트레이 앱입니다. Codex 리셋권은 만료 직전에 자동으로 써 줄 수도 있어요. 여러 계정을 등록할 수 있어요.

- 새 리셋권이 생기면 알림
- 만료 **72시간 전**, **24시간 전**에 알림 (정확한 만료 시각 기준, 한국 시간 표시)
- 알림의 [사용량 페이지 열기]로 바로 이동
- **Codex 자동 사용**: 리셋권 만료 **1시간 전**에 자동으로 써서 버려지지 않게 해요 (계정별 체크박스, 기본 켜짐)
  - 결과는 알림으로 알려줘요. 초기화할 사용량이 없으면 쓰이지 않고 그대로 만료돼요.
  - 그 시각에 PC가 켜져 있고 앱이 실행 중이어야 해요. 절전 중이었다면 깨어난 뒤 몇 분 안에 처리해요 (만료 전일 때만).
  - Claude 리셋권은 사용하지 않고 알림만 해요.

## 필요 조건

- Windows 10/11
- 쓰는 서비스의 공식 CLI
  - Claude: [Claude Code](https://code.claude.com) (`claude` 명령)
  - Codex: [Codex CLI](https://github.com/openai/codex) (`codex` 명령)

## 설치

1. [Releases](https://github.com/joseph-kang777/reset-noti/releases/latest)에서 `*-setup.exe`를 내려받아 실행합니다.
2. 코드 서명이 없어서 SmartScreen 경고가 뜰 수 있어요. **추가 정보 → 실행**을 누르세요.
3. 설치 후 자동 시작이 켜지고, 트레이에 아이콘이 생깁니다.
4. 새 버전은 앱 시작 시와 24시간마다 확인해요. 알림이 뜨면 앱 창 상단의 [설치]를 누르세요 (자동으로 설치되지는 않아요).

## 계정 추가

- **Claude**: [+ 계정 추가] → 브라우저에서 로그인하면 끝.
- **Codex**: [+ 계정 추가] → 앱에 뜬 코드를 브라우저 화면에 입력.
  - 브라우저에서 오류가 나면 [ChatGPT 보안 설정](https://chatgpt.com/#settings/Security)에서 **Codex 기기 코드 로그인**을 켜고 다시 시도하세요.

계정마다 앱 전용 로그인 폴더(`%APPDATA%\ResetNoti\accounts\`)를 써서, 평소 쓰는 `~/.claude`, `~/.codex` 로그인에는 영향을 주지 않아요. 별칭을 눌러 이름(예: 회사)을 붙이면 알림에 그 이름이 표시됩니다. Codex 계정 줄의 **만료 1시간 전 자동 사용** 체크를 끄면 그 계정은 알림만 받아요.

## 알아둘 점

- 두 서비스 모두 **공식 공개 API가 아닌** 내부 경로로 조회해요. CLI 업데이트로 동작이 바뀔 수 있고, 그때는 앱 업데이트로 고칩니다.
- Codex 자동 사용은 Codex CLI의 `app-server`(`account/rateLimitResetCredit/consume`)를 통해요. 리셋권마다 고정 키를 써서 재시도해도 두 번 쓰이지 않아요.
- Claude 로그인 토큰은 약 8시간마다 만료돼요. 앱이 `claude -p`를 아주 짧게 실행해 토큰을 갱신하므로 **계정 사용량이 아주 조금** 줄어요.
- Claude 리셋권의 정확한 응답 형태는 아직 실제로 확인하지 못했어요. 리셋권이 생기면 `%APPDATA%\ResetNoti\reset_noti.log`에 원문이 남으니 이슈로 알려주세요.

## 개발

```powershell
cd app
pnpm install
pnpm tauri dev                                   # 실행
cargo test --manifest-path src-tauri/Cargo.toml  # 테스트
```

### 릴리스

1. `app/src-tauri/tauri.conf.json`, `app/package.json`, `app/src-tauri/Cargo.toml`의 `version`을 올리고 커밋합니다.
2. `git push && git tag vX.Y.Z && git push origin vX.Y.Z` → GitHub Actions가 빌드·서명 후 Releases에 올립니다.

저장소 Secrets에 `TAURI_SIGNING_PRIVATE_KEY`(업데이트 서명 비밀 키)와 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`가 있어야 합니다. 비밀 키는 저장소에 넣지 마세요.

### 예전 Python 버전 제거

루트의 `reset_noti.py`는 초기 프로토타입입니다. 작업 스케줄러에 등록했다면 `powershell -File uninstall.ps1`로 제거하세요.
