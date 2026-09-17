// Local control panel logic.
//
// Talks to the Rust side over Tauri's IPC, which only this window is granted
// (see capabilities/local-panel.json). The remote site runs in a separate
// webview with no IPC at all, so nothing a server sends can reach these
// commands.
//
// State arrives two ways on purpose: one snapshot on open, then events. A
// panel that only polled would show a dropped connection seconds late, and one
// that only listened would open blank.

const { invoke } = window.__TAURI__.core
const { listen } = window.__TAURI__.event

const el = (id) => document.getElementById(id)

const fields = {
  siteUrl: el("siteUrl"),
  name: el("name"),
  workspace: el("workspace"),
  autoApprove: el("autoApprove"),
  connectOnLaunch: el("connectOnLaunch"),
  program: el("program"),
}

let lastSaved = null

function renderStatus(status, activeSessions) {
  const dot = el("dot")
  dot.dataset.state = status.state
  const text = {
    offline: "未连接",
    connecting: "正在连接…",
    online: "已连接",
    needs_login: "需要登录",
    error: "连接异常",
  }[status.state] ?? status.state

  let detail = ""
  if (status.state === "online") {
    detail = `${status.username || "已绑定"} · 设备 ${status.device_id}`
  } else if (status.state === "error") {
    detail = status.message
  } else if (status.state === "needs_login" && status.reason) {
    // A reason that merely restates the heading is noise; the server's own
    // wording is only worth showing when it says something more.
    detail = status.reason === text ? "" : status.reason
  }
  el("statusText").textContent = detail ? `${text} · ${detail}` : text

  // The sign-in form is shown exactly when a credential is what is missing;
  // leaving it up while connected invites a pointless re-login, and hiding it
  // when the token was revoked would leave the user stuck.
  el("loginCard").hidden = status.state !== "needs_login"

  el("sessions").textContent = activeSessions
    ? `· 本机正在运行 ${activeSessions} 个任务`
    : ""
}

function renderSettings(s) {
  fields.siteUrl.value = s.site_url
  el("siteLabel").textContent = s.site_url
  fields.name.value = s.name
  fields.workspace.value = s.workspace
  fields.autoApprove.checked = s.auto_approve
  fields.connectOnLaunch.checked = s.connect_on_launch
  fields.program.value = s.program
  el("autoWarn").hidden = !s.auto_approve
  lastSaved = JSON.stringify(collect())
}

function collect() {
  return {
    site_url: fields.siteUrl.value.trim(),
    name: fields.name.value.trim(),
    workspace: fields.workspace.value.trim(),
    auto_approve: fields.autoApprove.checked,
    connect_on_launch: fields.connectOnLaunch.checked,
    program: fields.program.value.trim() || "pi",
  }
}

function appendLog(line) {
  const log = el("log")
  // Only autoscroll when the user is already at the bottom, so reading older
  // output is not yanked away by new lines.
  const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 24
  log.textContent += (log.textContent ? "\n" : "") + line
  if (atBottom) log.scrollTop = log.scrollHeight
}

async function refresh() {
  const snap = await invoke("snapshot")
  renderStatus(snap.status, snap.active_sessions)
  renderSettings(snap.settings)
  el("log").textContent = snap.log.join("\n")
  el("log").scrollTop = el("log").scrollHeight
  el("boundAs").textContent = snap.bound_as
    ? `已绑定账号 ${snap.bound_as}，本机保存的是可单独吊销的设备令牌。`
    : "尚未绑定本机。"
}

function fail(e) {
  const message = typeof e === "string" ? e : (e?.message ?? String(e))
  const box = el("loginError")
  box.textContent = message
  box.hidden = false
  return message
}

el("save").addEventListener("click", async () => {
  const next = collect()
  try {
    const snap = await invoke("save_settings", { next })
    renderStatus(snap.status, snap.active_sessions)
    renderSettings(snap.settings)
    const saved = el("saved")
    saved.hidden = false
    setTimeout(() => (saved.hidden = true), 1600)
  } catch (e) {
    fail(e)
  }
})

el("pickWorkspace").addEventListener("click", async () => {
  const picked = await invoke("pick_workspace")
  if (picked) fields.workspace.value = picked
})

fields.autoApprove.addEventListener("change", () => {
  // Shown immediately, before saving: the consequence has to be visible while
  // the user still has their hand on the switch.
  el("autoWarn").hidden = !fields.autoApprove.checked
})

el("connect").addEventListener("click", async () => {
  // Connecting with unsaved edits would connect to something other than what
  // is on screen, so persist first.
  if (JSON.stringify(collect()) !== lastSaved) {
    try {
      await invoke("save_settings", { next: collect() })
      lastSaved = JSON.stringify(collect())
    } catch (e) {
      return void fail(e)
    }
  }
  try {
    await invoke("connect")
  } catch (e) {
    fail(e)
  }
})

el("disconnect").addEventListener("click", () => invoke("disconnect"))

el("signOut").addEventListener("click", async () => {
  await invoke("sign_out")
  await refresh()
})

el("signIn").addEventListener("click", async () => {
  const username = el("username").value
  const password = el("password").value
  el("loginError").hidden = true
  if (JSON.stringify(collect()) !== lastSaved) {
    try {
      await invoke("save_settings", { next: collect() })
      lastSaved = JSON.stringify(collect())
    } catch (e) {
      return void fail(e)
    }
  }
  if (!username || !password) {
    return void fail("账号和密码都不能为空")
  }
  try {
    await invoke("sign_in", { username, password })
    // Cleared right away: the field has no further use and a password left in
    // a DOM node is a password in a crash dump.
    el("password").value = ""
  } catch (e) {
    fail(e)
  }
})

el("password").addEventListener("keydown", (e) => {
  if (e.key === "Enter") el("signIn").click()
})

el("openSite").addEventListener("click", () => invoke("show_site"))
// Signing in on the site is the normal way to bind this machine, so the
// sign-in card points there first and keeps the password form folded away.
el("openSiteLogin").addEventListener("click", () => invoke("show_site"))

await listen("connector://status", async (event) => {
  // The session count lives on the Rust side, so a status change re-reads it
  // rather than guessing.
  const snap = await invoke("snapshot")
  renderStatus(event.payload, snap.active_sessions)
  el("boundAs").textContent = snap.bound_as
    ? `已绑定账号 ${snap.bound_as}，本机保存的是可单独吊销的设备令牌。`
    : "尚未绑定本机。"
})

await listen("connector://log", (event) => appendLog(event.payload))

await refresh()
