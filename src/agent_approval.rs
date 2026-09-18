//! What an agent must ask before it changes something.
//!
//! Shared by the server and the desktop client — literally the same file, the
//! way `runtime_env.rs` is — because the two halves of this policy have to
//! agree on more than a spelling: the server stores a per-task choice and the
//! client decides what to honour, so a mode that meant one thing on one side
//! and another on the other would be a security bug rather than a display bug.
//!
//! The gate itself is a pi extension: the runtime discovers extensions under
//! its own config directory, and `tool_call` is the only hook that can stop a
//! tool before it runs. Generating it here keeps one copy of the dialog text
//! for both execution targets, so a cloud task and a local task describe the
//! same command the same way.
//!
//! **A task may tighten this policy but never loosen it.** The machine's own
//! setting is a floor: the server forwards what the user picked for a task,
//! and the client resolves it with [`ApprovalMode::strictest`]. That is what
//! keeps "the policy that protects this computer" a decision of the computer,
//! even though the picker that changes it lives on a web page.

// One file, two crates, and neither uses all of it: the server only ever
// builds a `Cloud` gate while the client only ever builds a `Device` one, and
// `strictest` is the client's alone. Warning about that would only push the
// policy back into two copies, which is the thing this module exists to avoid.
#![allow(dead_code)]

/// How tool calls are gated.
///
/// Three steps rather than a switch, because "ask about everything" and "ask
/// about nothing" are both wrong most of the time: a task that edits twenty
/// files asks twenty times and the user stops reading the prompts, while
/// turning the gate off to get work done also hands over the shell. The
/// middle step keeps the expensive decision — running a command — in front of
/// a human while letting the agent write files inside the workspace it was
/// already confined to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask before every command, write and edit. The safe default.
    #[default]
    Always,
    /// Ask before commands; file writes and edits run on their own.
    Commands,
    /// Never ask. Only sensible where the damage is bounded by something else
    /// — a disposable container, or a machine the user treats as disposable.
    Never,
}

impl ApprovalMode {
    /// Parse a configured value, tolerating the old boolean spelling.
    ///
    /// `YUNOVA_DEVICE_AUTO_APPROVE=1` predates this setting and is still what
    /// existing service units pass, so it has to keep meaning what it meant:
    /// do not ask.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "always" | "ask" | "0" | "off" | "false" | "no" => Some(Self::Always),
            "commands" | "command" | "shell" => Some(Self::Commands),
            "never" | "auto" | "1" | "on" | "true" | "yes" => Some(Self::Never),
            _ => None,
        }
    }

    /// Tools that stop for a human under this mode.
    ///
    /// `read`/`grep`/`find` are deliberately absent at every level: they
    /// cannot change the machine, and gating them would bury the prompts that
    /// matter under ones nobody can answer usefully.
    pub fn gated(self) -> &'static [&'static str] {
        match self {
            Self::Always => &["bash", "powershell", "write", "edit"],
            Self::Commands => &["bash", "powershell"],
            Self::Never => &[],
        }
    }

    /// One line for a terminal or a log.
    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "逐条确认命令与文件改动",
            Self::Commands => "仅命令需要确认，文件改动自动放行",
            Self::Never => "全部自动执行",
        }
    }

    /// The wire spelling, matching the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Commands => "commands",
            Self::Never => "never",
        }
    }

    /// How much this mode stops, higher being stricter.
    ///
    /// Exists so the comparison below is a property of the policy rather than
    /// of the order the variants happen to be declared in.
    fn strictness(self) -> u8 {
        match self {
            Self::Never => 0,
            Self::Commands => 1,
            Self::Always => 2,
        }
    }

    /// The stricter of two policies.
    ///
    /// The whole reason a task may carry an approval mode at all: the machine
    /// the tools run on keeps the floor, and a per-task choice can only raise
    /// it. A task that asks for `never` on a computer configured to ask still
    /// asks — otherwise a compromised server, or anyone who could reach the
    /// task list, could silently turn off someone's shell confirmations.
    pub fn strictest(self, other: Self) -> Self {
        if other.strictness() > self.strictness() {
            other
        } else {
            self
        }
    }
}

/// Where the tools will run, for wording the dialog.
///
/// The prompt has to say *whose* machine is at stake: "允许执行命令" reads very
/// differently when the answer is a disposable container than when it is the
/// laptop the user is sitting at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The user's own computer, reached through the desktop client.
    Device,
    /// A per-session sandbox on the server.
    Cloud,
}

impl Scope {
    fn noun(self) -> &'static str {
        match self {
            Self::Device => "本机",
            Self::Cloud => "云电脑",
        }
    }
}

/// File name of the generated gate, inside a runtime's `extensions` directory.
pub const GATE_FILE: &str = "yunova-approval.js";

/// The approval gate, as a pi extension.
///
/// `None` for [`ApprovalMode::Never`]: an extension that gates nothing is
/// still an extension the runtime loads and the agent could read, and an empty
/// gate left on disk is indistinguishable from a gate that stopped working.
///
/// What it shows matters as much as what it blocks. The first version passed
/// an options object to `ctx.ui.confirm`, which takes `(title, message)`, and
/// read `event.args`, which pi calls `event.input` — so every prompt arrived
/// as a bare `{}` and the user was asked to approve a command they could not
/// see. An approval dialog that hides the action is not a safety feature; it
/// only teaches people to press 允许.
pub fn gate_source(mode: ApprovalMode, scope: Scope) -> Option<String> {
    let gated = mode.gated();
    if gated.is_empty() {
        return None;
    }
    let list = gated
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let where_ = scope.noun();
    Some(format!(
        r#"// Installed by Yunova. Gates tools that can change {where_}.
const GUARDED = new Set([{list}]);
const LIMIT = 1600;

const clip = (s) => {{
  const text = String(s ?? "");
  return text.length > LIMIT ? `${{text.slice(0, LIMIT)}}\n…（已截断）` : text;
}};
const firstLine = (s) => {{
  const [line = ""] = String(s ?? "").split("\n");
  return line.length > 120 ? `${{line.slice(0, 120)}}…` : line;
}};

function title(name) {{
  if (name === "bash" || name === "powershell") return "允许在{where_}执行命令？";
  if (name === "write") return "允许写入{where_}文件？";
  if (name === "edit") return "允许修改{where_}文件？";
  return `允许在{where_}执行 ${{name}}？`;
}}

// What the user is actually approving, per tool. A generic JSON dump is the
// fallback rather than the rule: the whole point is that the command or the
// path is readable at a glance on a phone.
function describe(name, input, cwd) {{
  const a = input ?? {{}};
  if (name === "bash" || name === "powershell") {{
    const where = cwd ? `目录：${{cwd}}\n\n` : "";
    return `${{where}}${{clip(a.command)}}`;
  }}
  if (name === "write") {{
    const body = typeof a.content === "string" ? a.content : "";
    const size = body ? `（${{body.length}} 字符）` : "";
    return `写入：${{a.path ?? "?"}}${{size}}\n\n${{clip(body)}}`;
  }}
  if (name === "edit") {{
    const edits = Array.isArray(a.edits) ? a.edits : [];
    const lines = edits.map(
      (e, i) => `${{i + 1}}. ${{firstLine(e?.oldText)}}\n   → ${{firstLine(e?.newText)}}`
    );
    return clip(`修改：${{a.path ?? "?"}}（${{edits.length}} 处）\n\n${{lines.join("\n")}}`);
  }}
  return clip(JSON.stringify(a, null, 2));
}}

export default function (pi) {{
  pi.on("tool_call", async (event, ctx) => {{
    if (!GUARDED.has(event.toolName)) return;
    // `input` is pi's own name for the arguments; `args` is kept only so an
    // older runtime still shows something rather than an empty dialog.
    const input = event.input ?? event.args;
    const ok = await ctx.ui.confirm(
      title(event.toolName),
      describe(event.toolName, input, ctx.cwd)
    );
    if (!ok) return {{ block: true, reason: "用户拒绝了该操作" }};
  }});
}}
"#
    ))
}

/// Install (or remove) the gate in a runtime's `extensions` directory.
///
/// Removing is as important as writing: a gate from a previous run would
/// silently contradict the current choice in both directions — a wider one
/// left behind asks about things the user stopped wanting asked about, and a
/// narrower one left behind is a boundary they believe is still there.
pub async fn write_gate(
    ext_dir: &std::path::Path,
    mode: ApprovalMode,
    scope: Scope,
) -> Result<(), String> {
    let path = ext_dir.join(GATE_FILE);
    match gate_source(mode, scope) {
        Some(source) => {
            tokio::fs::create_dir_all(ext_dir)
                .await
                .map_err(|e| format!("无法创建扩展目录: {e}"))?;
            tokio::fs::write(&path, source)
                .await
                .map_err(|e| format!("写入审批扩展失败: {e}"))
        }
        None => {
            let _ = tokio::fs::remove_file(&path).await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_boolean_setting_keeps_meaning_what_it_meant() {
        // `YUNOVA_DEVICE_AUTO_APPROVE=1` is in existing service units, and a
        // headless box that silently started asking for approvals nobody can
        // answer would simply stop doing work.
        assert_eq!(ApprovalMode::parse("1"), Some(ApprovalMode::Never));
        assert_eq!(ApprovalMode::parse("true"), Some(ApprovalMode::Never));
        assert_eq!(ApprovalMode::parse("0"), Some(ApprovalMode::Always));
        assert_eq!(
            ApprovalMode::parse("commands"),
            Some(ApprovalMode::Commands)
        );
        // An unreadable value is not silently read as "do not ask": the caller
        // keeps its default, which is to ask.
        assert_eq!(ApprovalMode::parse("sometimes"), None);
    }

    #[test]
    fn a_task_can_tighten_the_machines_policy_but_never_loosen_it() {
        use ApprovalMode::*;
        // The direction that must hold: whoever can create a task cannot turn
        // off the confirmations the owner of the machine asked for.
        assert_eq!(Always.strictest(Never), Always);
        assert_eq!(Commands.strictest(Never), Commands);
        assert_eq!(Always.strictest(Commands), Always);
        // The other direction is the feature: a machine left on "never" can
        // still be asked to confirm one particular task.
        assert_eq!(Never.strictest(Always), Always);
        assert_eq!(Never.strictest(Commands), Commands);
        // And it is idempotent, so replaying a start cannot drift.
        for m in [Always, Commands, Never] {
            assert_eq!(m.strictest(m), m);
        }
    }

    #[test]
    fn the_wire_spelling_round_trips() {
        for m in [
            ApprovalMode::Always,
            ApprovalMode::Commands,
            ApprovalMode::Never,
        ] {
            assert_eq!(ApprovalMode::parse(m.as_str()), Some(m));
        }
    }

    #[test]
    fn the_gate_names_the_machine_it_is_protecting() {
        // The same command is a different decision in a disposable container
        // than on the laptop the user is sitting at, so the dialog has to say
        // which one it is about.
        let device = gate_source(ApprovalMode::Always, Scope::Device).unwrap();
        assert!(device.contains("本机"), "got: {device}");
        let cloud = gate_source(ApprovalMode::Always, Scope::Cloud).unwrap();
        assert!(cloud.contains("云电脑"), "got: {cloud}");
        // An extension that gates nothing must not exist at all.
        assert!(gate_source(ApprovalMode::Never, Scope::Cloud).is_none());
    }

    #[tokio::test]
    async fn loosening_the_mode_removes_the_gate_rather_than_leaving_a_stale_one() {
        let dir = std::env::temp_dir().join(format!(
            "yunova-gate-{}-{}",
            std::process::id(),
            // Not `chrono`: this file is compiled into the desktop client too,
            // and that binary deliberately carries no dependency it does not
            // need on a machine users install it on.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let ext = dir.join(GATE_FILE);

        write_gate(&dir, ApprovalMode::Always, Scope::Cloud)
            .await
            .unwrap();
        assert!(ext.exists());

        write_gate(&dir, ApprovalMode::Never, Scope::Cloud)
            .await
            .unwrap();
        assert!(
            !ext.exists(),
            "a gate left behind is a boundary the user thinks is still there"
        );

        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
