import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";

type Service = "claude" | "codex";

interface AccountView {
  id: string;
  service: Service;
  label: string;
  alias: string | null;
  email: string | null;
  count: number;
  earliest: string | null;
  status: "ok" | "auth" | "error" | "unknown";
  error: string | null;
  last_ok: string | null;
}

const SECURITY_URL = "https://chatgpt.com/#settings/Security";
const $ = <T extends HTMLElement>(sel: string, root: ParentNode = document) => root.querySelector(sel) as T;

let deviceUrl = "";

function showToast(text: string) {
  const t = $<HTMLDivElement>("#toast");
  t.textContent = text;
  t.hidden = false;
  setTimeout(() => (t.hidden = true), 4000);
}

function confirmBox(title: string, text: string): Promise<boolean> {
  const box = $<HTMLDivElement>("#confirm");
  $<HTMLElement>("#confirm-title").textContent = title;
  $<HTMLElement>("#confirm-text").textContent = text;
  box.hidden = false;
  return new Promise((resolve) => {
    const done = (v: boolean) => {
      box.hidden = true;
      resolve(v);
    };
    $<HTMLButtonElement>("#confirm-yes").onclick = () => done(true);
    $<HTMLButtonElement>("#confirm-no").onclick = () => done(false);
  });
}

function line(a: AccountView): string {
  const who = a.email ?? a.label;
  if (a.count === 0) return `${who} · 리셋권 없음`;
  return `${who} · 리셋권 ${a.count}개` + (a.earliest ? ` · ${a.earliest} 만료` : "");
}

function badge(a: AccountView): HTMLElement | null {
  if (a.status === "auth") return el("span", "badge warn", "재인증 필요");
  if (a.status === "error") {
    const b = el("span", "badge err", "확인 실패");
    b.title = a.error ?? "";
    return b;
  }
  return null;
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function row(a: AccountView): HTMLLIElement {
  const li = el("li", "account") as HTMLLIElement;
  const info = el("div", "info");

  const name = el("div", "name");
  const alias = el("span", "alias", a.alias ?? "별칭 없음");
  if (!a.alias) alias.classList.add("muted");
  alias.title = "클릭해서 별칭 바꾸기";
  alias.addEventListener("click", () => editAlias(a, alias));
  name.append(alias);
  const b = badge(a);
  if (b) name.append(b);

  const detail = el("div", "detail", line(a));
  info.append(name, detail);
  if (a.last_ok) info.append(el("div", "sub muted", `마지막 확인 ${a.last_ok}`));

  const actions = el("div", "actions");
  const re = el("button", "btn small", "재인증");
  re.addEventListener("click", () => runLogin(a.service, () => invoke("reauth", { id: a.id }), "재인증했어요"));
  const rm = el("button", "btn small ghost", "제거");
  rm.addEventListener("click", async () => {
    if (await confirmBox(`${a.label} 계정을 제거할까요?`, "이 앱에 저장된 로그인 정보가 삭제돼요.")) {
      await invoke("remove_account", { id: a.id });
    }
  });
  actions.append(re, rm);
  li.append(info, actions);
  return li;
}

function editAlias(a: AccountView, span: HTMLElement) {
  const input = document.createElement("input");
  input.className = "alias-input";
  input.value = a.alias ?? "";
  input.placeholder = "예: 회사";
  span.replaceWith(input);
  input.focus();
  let done = false;
  const save = async () => {
    if (done) return;
    done = true;
    await invoke("rename_account", { id: a.id, alias: input.value.trim() || null });
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") save();
    if (e.key === "Escape") {
      done = true;
      render(lastViews);
    }
  });
  input.addEventListener("blur", save);
}

let lastViews: AccountView[] = [];

function render(views: AccountView[]) {
  lastViews = views;
  for (const svc of ["claude", "codex"] as Service[]) {
    const list = $<HTMLUListElement>(`section[data-service="${svc}"] .accounts`);
    list.replaceChildren();
    const mine = views.filter((v) => v.service === svc);
    if (mine.length === 0) list.append(el("li", "empty muted", "추가한 계정이 없어요."));
    for (const a of mine) list.append(row(a));
  }
}

async function runLogin(svc: Service, call: () => Promise<unknown>, okText: string) {
  const overlay = $<HTMLDivElement>("#login");
  $<HTMLElement>("#login-title").textContent =
    svc === "codex" ? "Codex 로그인 코드를 기다리는 중…" : "브라우저에서 Claude에 로그인해 주세요…";
  $<HTMLDivElement>("#device").hidden = true;
  overlay.hidden = false;
  try {
    await call();
    showToast(okText);
  } catch (e) {
    const msg = String(e);
    if (!msg.includes("취소")) showToast(msg);
  } finally {
    overlay.hidden = true;
  }
}

async function init() {
  const cli = await invoke<{ claude: boolean; codex: boolean }>("cli_status");
  for (const svc of ["claude", "codex"] as Service[]) {
    const ok = cli[svc];
    $<HTMLElement>(`section[data-service="${svc}"] .cli-missing`).hidden = ok;
    $<HTMLButtonElement>(`section[data-service="${svc}"] .add`).disabled = !ok;
  }

  document.querySelectorAll<HTMLButtonElement>("button.add").forEach((btn) => {
    const svc = btn.dataset.service as Service;
    btn.addEventListener("click", () => runLogin(svc, () => invoke("add_account", { service: svc }), "계정을 추가했어요"));
  });

  $<HTMLButtonElement>("#cancel-login").addEventListener("click", () => invoke("cancel_login"));
  $<HTMLButtonElement>("#copy-code").addEventListener("click", async () => {
    await navigator.clipboard.writeText($<HTMLElement>("#device-code").textContent ?? "");
    showToast("코드를 복사했어요");
  });
  $<HTMLButtonElement>("#open-device").addEventListener("click", () => openUrl(deviceUrl));
  $<HTMLAnchorElement>("#open-security").addEventListener("click", (e) => {
    e.preventDefault();
    openUrl(SECURITY_URL);
  });

  const check = $<HTMLButtonElement>("#check-now");
  check.addEventListener("click", async () => {
    check.disabled = true;
    check.textContent = "확인 중…";
    try {
      render(await invoke<AccountView[]>("check_now"));
    } finally {
      check.disabled = false;
      check.textContent = "지금 확인";
    }
  });

  const auto = $<HTMLInputElement>("#autostart");
  auto.checked = await invoke<boolean>("get_autostart");
  auto.addEventListener("change", () => invoke("set_autostart", { enabled: auto.checked }));

  await listen<AccountView[]>("accounts-changed", (e) => render(e.payload));
  await listen<{ url: string; code: string }>("login-event", (e) => {
    deviceUrl = e.payload.url;
    $<HTMLElement>("#device-code").textContent = e.payload.code;
    $<HTMLElement>("#login-title").textContent = "Codex 기기 코드로 로그인";
    $<HTMLDivElement>("#device").hidden = false;
    openUrl(deviceUrl);
  });

  const showUpdate = (v: string) => {
    $<HTMLElement>("#update-text").textContent = `새 버전 ${v}이(가) 있어요. 지금 설치할까요?`;
    $<HTMLDivElement>("#update").hidden = false;
  };
  await listen<string>("update-available", (e) => showUpdate(e.payload));
  $<HTMLButtonElement>("#install-update").addEventListener("click", async (e) => {
    const b = e.currentTarget as HTMLButtonElement;
    b.disabled = true;
    b.textContent = "설치 중…";
    try {
      await invoke("install_update");
    } catch (err) {
      showToast(String(err));
      b.disabled = false;
      b.textContent = "설치";
    }
  });
  invoke<string | null>("check_update").then((v) => v && showUpdate(v));

  render(await invoke<AccountView[]>("list_accounts"));
}

window.addEventListener("DOMContentLoaded", init);
