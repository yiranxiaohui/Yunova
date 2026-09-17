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
// Extra authorized directories. Held here rather than read back out of the
// DOM because the list is edited by picker and by button: one array is the
// state, and the list below is only a rendering of it.
let extraWorkspaces = []

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
  extraWorkspaces = Array.isArray(s.extra_workspaces) ? [...s.extra_workspaces] : []
  renderWorkspaces()
  fields.autoApprove.checked = s.auto_approve
  fields.connectOnLaunch.checked = s.connect_on_launch
  fields.program.value = s.program
  el("autoWarn").hidden = !s.auto_approve
  lastSaved = JSON.stringify(collect())
}

// The authorized list, default first.
//
// The default is shown alongside the extras, and without a remove button,
// because it cannot be removed: a machine always has one directory a task
// falls back to, and a list that implied otherwise would invite the user to
// try emptying it.
function renderWorkspaces() {
  const list = el("workspaceList")
  list.textContent = ""

  const row = (path, removable) => {
    const li = document.createElement("li")
    const code = document.createElement("code")
    code.textContent = path
    li.append(code)
    if (removable) {
      const remove = document.createElement("button")
      remove.type = "button"
      remove.className = "ghost danger"
      remove.textContent = "移除"
      remove.addEventListener("click", () => {
        extraWorkspaces = extraWorkspaces.filter((p) => p !== path)
        renderWorkspaces()
      })
      li.append(remove)
    } else {
      const tag = document.createElement("span")
      tag.className = "tag"
      tag.textContent = "默认"
      li.append(tag)
    }
    list.append(li)
  }

  const fallback = fields.workspace.value.trim()
  if (fallback) row(fallback, false)
  for (const path of extraWorkspaces) {
    // The default is already shown; listing it twice would look like a bug.
    if (path !== fallback) row(path, true)
  }
}

function collect() {
  return {
    site_url: fields.siteUrl.value.trim(),
    name: fields.name.value.trim(),
    workspace: fields.workspace.value.trim(),
    extra_workspaces: extraWorkspaces,
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

// The runtime section. Rendered from one status object rather than from
// several booleans so the panel cannot show "installed" and an install button
// at the same time.
function renderRuntime(runtime) {
  const version = el("runtimeVersion")
  const path = el("runtimePath")
  const install = el("installRuntime")
  const missing = el("runtimeMissing")

  version.textContent = runtime.installed ? `· ${runtime.version}` : ""
  // The resolved path matters on a machine with several installs: it answers
  // "which one will actually run", which is otherwise unknowable from here.
  path.textContent = runtime.path ? runtime.path : ""
  missing.hidden = runtime.installed

  const manager = runtime.installers[0]
  if (!manager) {
    // Nothing to install with. Saying so is better than offering a button that
    // can only fail: the user needs Node first, and that is not something this
    // app should try to install behind their back.
    install.hidden = true
    if (!runtime.installed) {
      missing.textContent =
        "未找到可用运行时，也没有找到 npm / bun / pnpm。请先安装 Node.js 或 Bun。"
    }
    return
  }
  install.hidden = false
  install.textContent = runtime.installed
    ? `用 ${manager} 更新运行时`
    : `用 ${manager} 安装运行时`
  install.dataset.manager = manager
}

async function refresh() {
  const snap = await invoke("snapshot")
  renderStatus(snap.status, snap.active_sessions)
  renderSettings(snap.settings)
  renderRuntime(snap.runtime)
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
  if (!picked) return
  fields.workspace.value = picked
  // The default appears in the list, so it has to be redrawn when it changes.
  renderWorkspaces()
})

el("addWorkspace").addEventListener("click", async () => {
  const picked = await invoke("pick_extra_workspace")
  if (!picked) return
  if (!extraWorkspaces.includes(picked)) extraWorkspaces.push(picked)
  renderWorkspaces()
})

// Typing a path by hand also changes the default, and the list has to follow.
fields.workspace.addEventListener("input", renderWorkspaces)

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

el("installRuntime").addEventListener("click", async (e) => {
  const button = e.currentTarget
  const manager = button.dataset.manager
  if (!manager) return
  const label = button.textContent
  // Disabled while it runs: an npm install takes tens of seconds, and a button
  // that still looks clickable invites a second concurrent install.
  button.disabled = true
  button.textContent = "正在安装…"
  el("runtimeError").hidden = true
  try {
    renderRuntime(await invoke("install_runtime", { manager }))
  } catch (err) {
    const box = el("runtimeError")
    box.textContent = typeof err === "string" ? err : (err?.message ?? String(err))
    box.hidden = false
  } finally {
    button.disabled = false
    button.textContent = label
  }
})

el("checkRuntime").addEventListener("click", async () => {
  // The user may have installed it in a terminal while this window was open,
  // so re-checking must be possible without restarting the app.
  el("runtimeError").hidden = true
  renderRuntime(await invoke("check_runtime"))
})
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
