from datetime import datetime, timedelta, timezone

from reset_noti import Grant, fmt_kst, parse_claude, parse_codex, plan

NOW = datetime(2026, 10, 23, 1, 0, tzinfo=timezone.utc)


def g(gid, hours_left, service="codex"):
    return Grant(service, gid, "Full reset", NOW + timedelta(hours=hours_left))


def titles(notes):
    return [n.title for n in notes]


def test_first_run_sends_one_summary_and_marks_seen():
    state = {}
    notes = plan(state, "codex", [g("a", 150), g("b", 300)], NOW)
    assert titles(notes) == ["Codex 리셋권 2개 보유 중"]
    assert "10/29(목) 16:00" in notes[0].body  # earliest expiry, KST
    assert plan(state, "codex", [g("a", 150), g("b", 300)], NOW) == []


def test_first_run_with_no_grants_is_silent():
    state = {}
    assert plan(state, "claude", [], NOW) == []
    assert plan(state, "claude", [g("x", 200, "claude")], NOW)[0].title == "Claude 리셋권이 생겼어요"


def test_new_grant_notifies_once():
    state = {}
    plan(state, "codex", [g("a", 150)], NOW)
    notes = plan(state, "codex", [g("a", 150), g("b", 300)], NOW)
    assert titles(notes) == ["Codex 리셋권이 생겼어요"]
    assert "현재 2개 보유" in notes[0].body
    assert plan(state, "codex", [g("a", 150), g("b", 300)], NOW) == []


def test_expiry_alerts_at_72h_then_24h_once_each():
    state = {}
    plan(state, "codex", [g("a", 100)], NOW)
    assert plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=27)) == []
    notes = plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=28))
    assert titles(notes) == ["Codex 리셋권 72시간 남음"]
    assert plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=29)) == []
    notes = plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=76))
    assert titles(notes) == ["Codex 리셋권 24시간 남음"]
    assert plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=80)) == []


def test_missed_72h_window_only_sends_24h():
    state = {}
    plan(state, "codex", [g("a", 100)], NOW)
    notes = plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=80))
    assert titles(notes) == ["Codex 리셋권 24시간 남음"]
    assert plan(state, "codex", [g("a", 100)], NOW + timedelta(hours=81)) == []


def test_new_grant_already_near_expiry_does_not_double_alert():
    state = {}
    plan(state, "codex", [], NOW)
    assert titles(plan(state, "codex", [g("a", 20)], NOW)) == ["Codex 리셋권이 생겼어요"]
    assert plan(state, "codex", [g("a", 20)], NOW + timedelta(hours=1)) == []


def test_grant_without_expiry_never_gets_expiry_alert():
    state = {}
    plan(state, "claude", [], NOW)
    grant = Grant("claude", "x", "리셋", None)
    assert titles(plan(state, "claude", [grant], NOW)) == ["Claude 리셋권이 생겼어요"]
    assert plan(state, "claude", [grant], NOW + timedelta(days=30)) == []


def test_fmt_kst():
    assert fmt_kst(datetime(2026, 10, 29, 19, 13, tzinfo=timezone.utc)) == "10/30(금) 04:13"


def test_parse_codex_keeps_only_available():
    data = {"credits": [
        {"id": "c1", "status": "available", "title": "Full reset", "expires_at": "2026-10-29T19:13:31.690054Z"},
        {"id": "c2", "status": "redeemed", "title": "Full reset", "expires_at": "2026-11-06T23:41:26Z"},
    ]}
    grants = parse_codex(data)
    assert [x.id for x in grants] == ["c1"]
    assert grants[0].expires_at == datetime(2026, 10, 29, 19, 13, 31, 690054, tzinfo=timezone.utc)


def test_parse_claude_tolerates_unknown_shapes():
    assert parse_claude({"cedar_ember": None}) == []
    assert parse_claude({"cedar_ember": {"grants": []}}) == []
    grants = parse_claude({"cedar_ember": {"grants": [
        {"grant_id": "g1", "kind": "weekly", "expires_at": "2026-11-01T00:00:00+00:00"},
        "g2",
    ]}})
    assert [(x.id, x.title) for x in grants] == [("g1", "weekly reset"), ("g2", "리셋")]
    assert grants[0].expires_at == datetime(2026, 11, 1, tzinfo=timezone.utc)
    assert grants[1].expires_at is None
