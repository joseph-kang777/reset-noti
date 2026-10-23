"""Claude / Codex 리셋권 알림.

1회 실행 후 종료한다. 작업 스케줄러가 1시간마다 실행(install.ps1).
- 새 리셋권이 생기면 알림
- 만료 72시간 전, 24시간 전에 알림
리셋권을 사용(consume)하는 요청은 절대 보내지 않는다. 조회만 한다.
"""
import base64
import json
import logging
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from xml.sax.saxutils import escape

HERE = Path(__file__).resolve().parent
STATE_FILE = HERE / "state.json"
LOG_FILE = HERE / "reset_noti.log"
ASSETS = HERE / "assets"
HOME = Path.home()

KST = timezone(timedelta(hours=9))
WEEKDAYS = "월화수목금토일"
THRESHOLDS_H = (72, 24)
AUMID = "ResetNoti"

SERVICES = {
    "claude": {"name": "Claude", "url": "https://claude.ai/settings/usage", "color": (217, 119, 87)},
    "codex": {"name": "Codex", "url": "https://chatgpt.com/codex/settings/usage", "color": (16, 163, 127)},
}

log = logging.getLogger("reset_noti")


@dataclass(frozen=True)
class Grant:
    service: str
    id: str
    title: str
    expires_at: datetime | None


@dataclass(frozen=True)
class Note:
    service: str
    title: str
    body: str


class AuthError(Exception):
    pass


# ---------------------------------------------------------------- 시간/파싱

def fmt_kst(dt: datetime) -> str:
    k = dt.astimezone(KST)
    return f"{k.month}/{k.day}({WEEKDAYS[k.weekday()]}) {k:%H:%M}"


def parse_time(value) -> datetime | None:
    if value is None:
        return None
    try:
        if isinstance(value, (int, float)):
            return datetime.fromtimestamp(value / 1000 if value > 1e12 else value, timezone.utc)
        dt = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
        return dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)
    except (ValueError, OverflowError, OSError):
        return None


def parse_codex(data: dict) -> list[Grant]:
    return [
        Grant("codex", c["id"], c.get("title") or "리셋", parse_time(c.get("expires_at")))
        for c in data.get("credits") or []
        if c.get("status") == "available" and c.get("id")
    ]


def parse_claude(data: dict) -> list[Grant]:
    # Claude 리셋권 응답 형태는 실제 리셋권으로 확인하지 못해서 최대한 관대하게 읽는다.
    grants = []
    for raw in (data.get("cedar_ember") or {}).get("grants") or []:
        if isinstance(raw, dict):
            gid = raw.get("grant_id") or raw.get("id")
            kind = raw.get("kind") or raw.get("reset_type") or raw.get("type")
            expires = next((raw[k] for k in ("expires_at", "expiresAt", "expires", "valid_until") if raw.get(k)), None)
            if gid:
                grants.append(Grant("claude", str(gid), f"{kind} reset" if kind else "리셋", parse_time(expires)))
        elif isinstance(raw, str):
            grants.append(Grant("claude", raw, "리셋", None))
    return grants


# ---------------------------------------------------------------- 알림 판단

def _due_thresholds(grant: Grant, now: datetime) -> list[int]:
    if grant.expires_at is None:
        return []
    left = grant.expires_at - now
    if left <= timedelta(0):
        return []
    return [h for h in THRESHOLDS_H if left <= timedelta(hours=h)]


def plan(state: dict, service: str, grants: list[Grant], now: datetime) -> list[Note]:
    """보낼 알림 목록을 만들고 state를 갱신한다."""
    name = SERVICES[service]["name"]
    first_run = service not in state
    svc = state.setdefault(service, {"seen": [], "sent": []})
    seen, sent = set(svc["seen"]), set(svc["sent"])
    notes = []

    new = [g for g in grants if g.id not in seen]
    if first_run and grants:
        dated = [g.expires_at for g in grants if g.expires_at]
        body = f"가장 빠른 만료: {fmt_kst(min(dated))}" if dated else "만료 시각 정보 없음"
        notes.append(Note(service, f"{name} 리셋권 {len(grants)}개 보유 중", body))
    elif not first_run:
        for g in new:
            until = f" · {fmt_kst(g.expires_at)}까지" if g.expires_at else ""
            notes.append(Note(service, f"{name} 리셋권이 생겼어요", f"{g.title}{until}\n현재 {len(grants)}개 보유"))

    new_ids = {g.id for g in new}
    for g in grants:
        due = _due_thresholds(g, now)
        if not due:
            continue
        keys = {f"{g.id}:{h}" for h in due}
        nearest = min(due)
        if g.id not in new_ids and f"{g.id}:{nearest}" not in sent:
            tail = " · 안 쓰면 사라져요" if nearest == 24 else ""
            notes.append(Note(service, f"{name} 리셋권 {nearest}시간 남음", f"{fmt_kst(g.expires_at)} 만료{tail}"))
        sent |= keys

    svc["seen"] = sorted(seen | {g.id for g in grants})
    svc["sent"] = sorted(sent)
    return notes


# ---------------------------------------------------------------- 조회

def _get(url: str, headers: dict) -> dict:
    req = urllib.request.Request(url, headers={"Accept": "application/json", "User-Agent": "reset-noti", **headers})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        if e.code in (401, 403):
            raise AuthError(f"HTTP {e.code}") from None
        raise


def fetch_claude() -> list[Grant]:
    oauth = json.loads((HOME / ".claude/.credentials.json").read_text(encoding="utf-8")).get("claudeAiOauth") or {}
    token, expires = oauth.get("accessToken"), parse_time(oauth.get("expiresAt"))
    if not token or (expires and expires < datetime.now(timezone.utc)):
        raise AuthError("Claude 로그인 토큰 만료")
    data = _get("https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1",
                {"Authorization": f"Bearer {token}", "anthropic-beta": "oauth-2025-04-20"})
    ember = data.get("cedar_ember")
    if ember and ember.get("grants"):
        log.info("claude cedar_ember: %s", json.dumps(ember, ensure_ascii=False))
    return parse_claude(data)


def fetch_codex() -> list[Grant]:
    tokens = json.loads((HOME / ".codex/auth.json").read_text(encoding="utf-8")).get("tokens") or {}
    if not tokens.get("access_token"):
        raise AuthError("Codex 로그인 정보 없음")
    data = _get("https://chatgpt.com/backend-api/wham/rate-limit-reset-credits",
                {"Authorization": f"Bearer {tokens['access_token']}", "ChatGPT-Account-ID": tokens.get("account_id", "")})
    return parse_codex(data)


FETCHERS = {"claude": fetch_claude, "codex": fetch_codex}


# ---------------------------------------------------------------- Windows 알림

def _icon(service: str) -> Path:
    path = ASSETS / f"{service}.png"
    if path.exists():
        return path
    from PIL import Image, ImageDraw  # 아이콘이 없을 때만 필요

    ASSETS.mkdir(exist_ok=True)
    img = Image.new("RGBA", (128, 128), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((0, 0, 127, 127), radius=24, fill=SERVICES[service]["color"])
    if service == "claude":  # 방사형 별
        for i in range(4):
            dx, dy = [(44, 0), (0, 44), (31, 31), (31, -31)][i]
            d.line((64 - dx, 64 - dy, 64 + dx, 64 + dy), fill="white", width=12)
    else:  # >_
        d.line((34, 40, 58, 64, 34, 88), fill="white", width=12, joint="curve")
        d.line((66, 90, 96, 90), fill="white", width=12)
    img.save(path)
    return path


def _register_app() -> None:
    import winreg

    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, rf"Software\Classes\AppUserModelId\{AUMID}") as k:
        winreg.SetValueEx(k, "DisplayName", 0, winreg.REG_SZ, "리셋권 알림")
        winreg.SetValueEx(k, "IconUri", 0, winreg.REG_SZ, str(_icon("codex")))


def show_toast(note: Note) -> None:
    url = escape(SERVICES[note.service]["url"], {'"': "&quot;"})
    lines = "".join(f"<text>{escape(t)}</text>" for t in [note.title, *note.body.split("\n")])
    xml = (
        f'<toast activationType="protocol" launch="{url}"><visual><binding template="ToastGeneric">{lines}'
        f'<image placement="appLogoOverride" src="{_icon(note.service).as_uri()}"/></binding></visual>'
        f'<actions><action content="사용량 페이지 열기" activationType="protocol" arguments="{url}"/>'
        f'<action content="닫기" activationType="system" arguments="dismiss"/></actions></toast>'
    )
    ps = f"""
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null
$x = New-Object Windows.Data.Xml.Dom.XmlDocument
$x.LoadXml([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{base64.b64encode(xml.encode()).decode()}')))
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{AUMID}').Show([Windows.UI.Notifications.ToastNotification]::new($x))
"""
    subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand",
         base64.b64encode(ps.encode("utf-16-le")).decode()],
        check=True, capture_output=True, timeout=60, creationflags=subprocess.CREATE_NO_WINDOW,
    )


# ---------------------------------------------------------------- main

def run(now: datetime | None = None) -> None:
    now = now or datetime.now(timezone.utc)
    state = json.loads(STATE_FILE.read_text(encoding="utf-8")) if STATE_FILE.exists() else {}
    _register_app()
    for service, fetch in FETCHERS.items():
        name = SERVICES[service]["name"]
        try:
            grants = fetch()
        except FileNotFoundError:
            log.info("%s: 로그인 파일 없음, 건너뜀", name)
            continue
        except AuthError as e:
            log.warning("%s: %s", name, e)
            today = now.astimezone(KST).date().isoformat()
            fails = state.setdefault("auth_fail_notified", {})
            if fails.get(service) != today:
                fails[service] = today
                show_toast(Note(service, f"{name} 리셋권 확인 실패",
                                f"로그인이 만료됐어요. {name}을(를) 한 번 실행해 주세요."))
            continue
        except Exception:
            log.exception("%s: 조회 실패", name)
            continue
        log.info("%s: 리셋권 %d개", name, len(grants))
        for note in plan(state, service, grants, now):
            log.info("알림: %s | %s", note.title, note.body.replace("\n", " / "))
            show_toast(note)
    STATE_FILE.write_text(json.dumps(state, ensure_ascii=False, indent=2), encoding="utf-8")


def main() -> None:
    logging.basicConfig(filename=LOG_FILE, encoding="utf-8", level=logging.INFO,
                        format="%(asctime)s %(levelname)s %(message)s")
    if "--test" in sys.argv:
        _register_app()
        show_toast(Note("codex", "Codex 리셋권 24시간 남음", "10/30(금) 04:13 만료 · 안 쓰면 사라져요"))
        show_toast(Note("claude", "Claude 리셋권이 생겼어요", "weekly reset · 11/1(일) 09:00까지\n현재 1개 보유"))
        return
    try:
        run()
    except Exception:
        log.exception("실행 실패")
        raise


if __name__ == "__main__":
    main()
