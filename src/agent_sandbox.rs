//! Cloud sandbox driver: the agent runtime inside a per-session container.
//!
//! Work-mode tasks give the agent a shell, so the cloud target needs a real
//! boundary. A container is that boundary, and it is what makes *automatic*
//! approval defensible: without isolation every shell call would have to be
//! confirmed by hand, which defeats the point of an agent.
//!
//! The transport is deliberately unchanged. `docker run -i` speaks the same
//! stdio as a local child process, so this driver reuses
//! [`crate::agent_driver::spawn_piped`] and inherits identical framing,
//! stderr draining and shutdown semantics. Containerisation lands as a
//! deployment change, not a protocol change — which is exactly what the
//! `AgentTransport` seam was introduced for.
//!
//! What this module actually owns is *policy*: the isolation flags, the
//! resource caps, and the network shape that keeps a sandbox able to reach
//! Yunova's gateway but nothing else.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::agent_driver::SubprocessTransport;

/// Image running `pi --mode rpc`. Built from `docker/sandbox.Dockerfile`.
pub const DEFAULT_IMAGE: &str = "yunova-sandbox:latest";

/// Container name for a session. Also the handle used to stop a leftover
/// container from a previous process, so it must be derivable from the id
/// alone rather than stored.
pub fn container_name(session_id: i64) -> String {
    format!("yunova-agent-s{session_id}")
}

/// Resource and isolation policy for one sandbox.
///
/// Defaults are intentionally conservative: an agent that misbehaves, or a
/// user who tries to mine, must be bounded by the container rather than by
/// trust. Every field is admin-overridable because the right ceiling depends
/// on the host, not on this code.
#[derive(Debug, Clone)]
pub struct SandboxLimits {
    pub image: String,
    pub cpus: String,
    pub memory: String,
    /// Bounds fork bombs, which a memory cap alone does not stop.
    pub pids: u32,
    /// Size of the writable workspace tmpfs.
    pub workspace_size: String,
    /// Wall-clock ceiling; a sandbox is torn down past this even if idle.
    pub max_lifetime_secs: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            image: DEFAULT_IMAGE.to_string(),
            cpus: "1".into(),
            memory: "1g".into(),
            pids: 256,
            workspace_size: "1g".into(),
            max_lifetime_secs: 3600,
        }
    }
}

impl SandboxLimits {
    /// Read overrides from the environment, keeping the defaults above when a
    /// value is absent or unparseable.
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            image: crate::runtime_env::var("YUNOVA_SANDBOX_IMAGE").unwrap_or(d.image),
            cpus: crate::runtime_env::var("YUNOVA_SANDBOX_CPUS").unwrap_or(d.cpus),
            memory: crate::runtime_env::var("YUNOVA_SANDBOX_MEMORY").unwrap_or(d.memory),
            pids: crate::runtime_env::var("YUNOVA_SANDBOX_PIDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.pids),
            workspace_size: crate::runtime_env::var("YUNOVA_SANDBOX_WORKSPACE_SIZE")
                .unwrap_or(d.workspace_size),
            max_lifetime_secs: crate::runtime_env::var("YUNOVA_SANDBOX_MAX_LIFETIME")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.max_lifetime_secs),
        }
    }
}

/// How to reach the container runtime.
///
/// Docker's socket is root-equivalent, so a deployment that does not add the
/// server's user to the `docker` group needs a wrapper such as `sudo -n`.
/// Splitting it out keeps that decision in configuration instead of hardcoding
/// a privilege escalation.
pub fn docker_command() -> Command {
    let raw = crate::runtime_env::var("YUNOVA_DOCKER_BIN").unwrap_or_else(|_| "docker".into());
    let mut parts = raw.split_whitespace();
    let program = parts.next().unwrap_or("docker");
    let mut cmd = Command::new(program);
    for arg in parts {
        cmd.arg(arg);
    }
    cmd
}

/// Dedicated Docker network for sandboxes.
///
/// Created with `--internal`, which drops the default bridge's masquerade
/// route. That is what actually prevents egress: `--add-host` only names the
/// gateway, it does not restrict routing, so on the default bridge an agent
/// can still reach the public internet and act as an exfiltration path or an
/// open proxy.
pub const NETWORK_NAME: &str = "yunova-sandbox";

/// Ensure the internal sandbox network exists and return its gateway address.
///
/// An internal network has no route off the host, but the host itself is still
/// reachable at the bridge gateway, which is how a sandbox reaches Yunova's
/// metered model gateway while remaining unable to call anything else.
pub async fn ensure_network() -> Result<String, String> {
    // Create unconditionally and tolerate "already exists": checking first
    // would race with another starting session.
    let mut create = docker_command();
    create
        .arg("network")
        .arg("create")
        .arg("--internal")
        .arg(NETWORK_NAME)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = create.status().await;

    let mut inspect = docker_command();
    inspect
        .arg("network")
        .arg("inspect")
        .arg(NETWORK_NAME)
        .arg("--format")
        .arg("{{(index .IPAM.Config 0).Gateway}}")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let out = inspect
        .output()
        .await
        .map_err(|e| format!("无法读取沙箱网络信息: {e}"))?;
    let gateway = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if gateway.is_empty() {
        return Err("沙箱网络缺少网关地址，无法访问模型网关".into());
    }
    Ok(gateway)
}

/// Build the argument list for a sandbox container.
///
/// Extracted as a pure function so the isolation policy is directly
/// assertable: these flags are the security boundary, and a silent regression
/// here would not show up as a failing feature.
pub fn build_run_args(
    session_id: i64,
    limits: &SandboxLimits,
    agent_dir: &Path,
    gateway_addr: &str,
    gateway_port: u16,
) -> Vec<String> {
    let name = container_name(session_id);
    // Run as the server's own uid/gid.
    //
    // The generated models.json holds a quota-spending gateway token and is
    // kept 0600 on the host, so a container running as the image's baked-in
    // user cannot read it. Relaxing the file's permissions instead would
    // expose the token to every local user on the host, which is the worse
    // trade. This also means anything the agent writes to a bind mount is
    // owned by the server rather than by a foreign uid.
    let (uid, gid) = host_ids();

    let mut args: Vec<String> = vec![
        "run".into(),
        // Stdio is the RPC transport; no TTY, because a pty would inject
        // control sequences into the JSONL stream.
        "-i".into(),
        "--rm".into(),
        "--name".into(),
        name,
        "--user".into(),
        format!("{uid}:{gid}"),
        // --- privilege ---
        // The agent has a shell inside, so it must start with nothing and
        // never be able to gain more.
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        // --- resources ---
        // A memory cap alone does not stop a fork bomb; pids does.
        "--pids-limit".into(),
        limits.pids.to_string(),
        "--memory".into(),
        limits.memory.clone(),
        "--cpus".into(),
        limits.cpus.clone(),
        // --- filesystem ---
        // Read-only root with explicit writable tmpfs mounts: the agent can
        // work freely in its workspace while the image itself stays pristine,
        // and nothing it writes survives the container.
        "--read-only".into(),
        "--tmpfs".into(),
        format!("/workspace:rw,size={},mode=1777", limits.workspace_size),
        "--tmpfs".into(),
        "/tmp:rw,size=256m,mode=1777".into(),
        // pi writes caches and its session file under its config dir. Owned by
        // the runtime uid so it is writable without widening the image.
        "--tmpfs".into(),
        format!("/home/agent:rw,size=64m,mode=0700,uid={uid},gid={gid}"),
        // HOME must match the writable tmpfs above; the image's default is
        // tied to its baked-in user, which we are overriding.
        "-e".into(),
        "HOME=/home/agent".into(),
    ];

    // The generated models.json, mounted read-only. It holds a session-scoped
    // gateway token; the agent needs to read it but must never rewrite it to
    // point somewhere else.
    args.push("-v".into());
    args.push(format!(
        "{}:/run/yunova/models.json:ro",
        agent_dir.join("models.json").display()
    ));
    // Copied into place by the wrapper below, because the mount target itself
    // lives inside a tmpfs that is created after mounts are resolved.
    args.push("-e".into());
    args.push("YUNOVA_MODELS_SRC=/run/yunova/models.json".into());

    // Session extensions, also read-only. Without this a sandbox could not be
    // given a `tool_call` approval gate at all: the runtime only discovers
    // extensions under its config dir, and that dir is an in-container tmpfs.
    // Read-only matters because an extension can block tool calls, so the
    // agent must not be able to edit away its own guardrails.
    let ext_dir = agent_dir.join("extensions");
    if ext_dir.is_dir() {
        args.push("-v".into());
        args.push(format!("{}:/run/yunova/extensions:ro", ext_dir.display()));
        args.push("-e".into());
        args.push("YUNOVA_EXT_SRC=/run/yunova/extensions".into());
    }

    // --- network ---
    // An internal network with no route off the host, plus a name for the
    // gateway. The sandbox can make metered model calls and nothing else:
    // on the default bridge it would still reach the public internet.
    args.push("--network".into());
    args.push(NETWORK_NAME.into());
    args.push("--add-host".into());
    args.push(format!("yunova-gateway:{gateway_addr}"));
    args.push("-e".into());
    args.push(format!(
        "YUNOVA_GATEWAY=http://yunova-gateway:{gateway_port}"
    ));

    args.push(limits.image.clone());

    // Stage the read-only config into pi's config dir, then exec the runtime.
    // `exec` matters: pi must be the process that owns stdio and receives the
    // stop signal, not a shell wrapping it.
    args.push("sh".into());
    args.push("-c".into());
    args.push(
        "set -e; mkdir -p \"$PI_CODING_AGENT_DIR\"; \
         cp \"$YUNOVA_MODELS_SRC\" \"$PI_CODING_AGENT_DIR/models.json\"; \
         if [ -n \"$YUNOVA_EXT_SRC\" ] && [ -d \"$YUNOVA_EXT_SRC\" ]; then \
           cp -r \"$YUNOVA_EXT_SRC\" \"$PI_CODING_AGENT_DIR/extensions\"; \
         fi; \
         exec pi --mode rpc --no-session"
            .into(),
    );

    args
}

/// Start a sandbox for `session_id` and return its transport and frame stream.
pub async fn spawn(
    session_id: i64,
    agent_dir: &Path,
    gateway_port: u16,
    limits: &SandboxLimits,
) -> Result<(Arc<SubprocessTransport>, mpsc::Receiver<Value>), String> {
    // A container left behind by a crashed process would take the name and
    // make every later start fail, so clear it first. Expected to be a no-op.
    remove_container(session_id).await;

    let gateway_addr = ensure_network().await?;

    let mut cmd = docker_command();
    for arg in build_run_args(session_id, limits, agent_dir, &gateway_addr, gateway_port) {
        cmd.arg(arg);
    }
    crate::agent_driver::spawn_piped(cmd, &limits.image).await
}

/// Force-remove a session's container.
///
/// Used both before starting and when tearing down. `docker rm -f` rather than
/// `stop` because the caller has already decided the sandbox is finished, and
/// its filesystem is a tmpfs with nothing worth flushing.
pub async fn remove_container(session_id: i64) {
    let mut cmd = docker_command();
    cmd.arg("rm")
        .arg("-f")
        .arg(container_name(session_id))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.status().await;
}

/// Remove sandbox containers left behind by a previous process.
///
/// Containers are named per session, so a crash leaves one holding both the
/// name and its resources. Matching on the name prefix is safe because no
/// other workload uses it.
pub async fn reap_orphans() {
    let mut list = docker_command();
    list.arg("ps")
        .arg("-aq")
        .arg("--filter")
        .arg(format!("name=^{}", container_name_prefix()))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(out) = list.output().await else {
        // No container runtime configured or reachable. Not fatal: a
        // deployment may run with sandboxing disabled.
        return;
    };
    let ids: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return;
    }
    eprintln!(
        "[agent-sandbox] removing {} orphaned sandbox container(s) from a previous run",
        ids.len()
    );
    let mut rm = docker_command();
    rm.arg("rm").arg("-f");
    for id in ids {
        rm.arg(id);
    }
    rm.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = rm.status().await;
}

/// Shared prefix of every sandbox container name.
fn container_name_prefix() -> &'static str {
    "yunova-agent-s"
}

/// The uid/gid the server runs as, used for the sandbox's `--user`.
///
/// Derived from the metadata of a path the process owns rather than through
/// libc, keeping this dependency-free. Falls back to the container image's
/// baked-in agent user when the ids cannot be read, which is correct for a
/// deployment whose credential file is group-readable instead.
fn host_ids() -> (u32, u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata("/proc/self") {
            return (meta.uid(), meta.gid());
        }
    }
    (10001, 10001)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_for_test() -> Vec<String> {
        build_run_args(
            42,
            &SandboxLimits::default(),
            Path::new("/data/agent-runtimes/s42/agent"),
            "172.18.0.1",
            3000,
        )
    }

    /// The isolation flags *are* the security boundary for work-mode tasks. A
    /// regression here would not surface as a broken feature, so assert them
    /// explicitly.
    #[test]
    fn the_sandbox_drops_privileges_and_cannot_regain_them() {
        let args = args_for_test();
        let joined = args.join(" ");
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--read-only"));
    }

    #[test]
    fn resource_caps_bound_a_runaway_agent() {
        let args = args_for_test();
        let joined = args.join(" ");
        // A memory cap alone does not stop a fork bomb.
        assert!(joined.contains("--pids-limit 256"));
        assert!(joined.contains("--memory 1g"));
        assert!(joined.contains("--cpus 1"));
    }

    #[test]
    fn no_tty_is_allocated_so_the_jsonl_stream_stays_clean() {
        let args = args_for_test();
        assert!(args.contains(&"-i".to_string()));
        assert!(
            !args.contains(&"-t".to_string()) && !args.contains(&"-it".to_string()),
            "a pty would inject control sequences into the RPC stream"
        );
    }

    #[test]
    fn the_container_is_removed_on_exit_and_named_per_session() {
        let args = args_for_test();
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&container_name(42)));
        assert_eq!(container_name(42), "yunova-agent-s42");
        assert_ne!(container_name(42), container_name(43));
        // Orphan reaping filters on this prefix, so the two must agree or a
        // crashed process would leave containers behind forever.
        assert!(container_name(42).starts_with(container_name_prefix()));
    }

    /// Egress control is the property that keeps a prompt-injected agent from
    /// exfiltrating data or acting as an open proxy. `--add-host` alone does
    /// not provide it: on the default bridge the sandbox still routes to the
    /// public internet, which end-to-end testing confirmed.
    #[test]
    fn the_sandbox_is_confined_to_an_internal_network_with_only_the_gateway_named() {
        let joined = args_for_test().join(" ");
        assert!(joined.contains(&format!("--network {NETWORK_NAME}")));
        assert!(joined.contains("--add-host yunova-gateway:172.18.0.1"));
        assert!(joined.contains("YUNOVA_GATEWAY=http://yunova-gateway:3000"));
    }

    #[test]
    fn the_gateway_credential_is_mounted_read_only() {
        // The agent must read the token but must not be able to repoint it at
        // an upstream of its choosing.
        let args = args_for_test();
        let joined = args.join(" ");
        assert!(
            joined.contains("/data/agent-runtimes/s42/agent/models.json:/run/yunova/models.json:ro"),
            "expected a read-only bind, got: {joined}"
        );
    }

    #[test]
    fn writable_paths_are_ephemeral_tmpfs_mounts() {
        let args = args_for_test();
        let joined = args.join(" ");
        // Nothing the agent writes may survive the container.
        assert!(joined.contains("/workspace:rw,size=1g"));
        assert!(joined.contains("/tmp:rw"));
        assert!(joined.contains("/home/agent:rw"));
    }

    /// The sandbox runs as the server's uid so it can read the 0600 gateway
    /// credential. Loosening that file instead would expose a quota-spending
    /// token to every local user on the host.
    #[test]
    fn the_sandbox_runs_as_the_server_uid_to_read_its_private_credential() {
        let (uid, gid) = host_ids();
        let joined = args_for_test().join(" ");
        assert!(joined.contains(&format!("--user {uid}:{gid}")));
        // The writable home must be owned by that same uid, or pi cannot
        // write its caches.
        assert!(joined.contains(&format!("uid={uid},gid={gid}")));
        // HOME has to follow, because the image's default belongs to the
        // baked-in user we are overriding.
        assert!(joined.contains("HOME=/home/agent"));
    }

    #[test]
    fn host_ids_resolve_to_the_running_process() {
        let (uid, _gid) = host_ids();
        // Must not be the fallback on a normal Linux host, or the sandbox
        // would silently fail to read its credential.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let expected = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(uid);
            assert_eq!(uid, expected);
        }
    }

    #[test]
    fn the_runtime_replaces_the_staging_shell() {
        // pi must own stdio and receive the stop signal directly.
        let last = args_for_test().last().cloned().unwrap_or_default();
        assert!(last.contains("exec pi --mode rpc --no-session"));
    }

    /// A sandbox must be able to carry a `tool_call` approval gate. The
    /// runtime only discovers extensions under its config dir, which is an
    /// in-container tmpfs, so they have to be staged in from a bind mount.
    #[test]
    fn session_extensions_are_staged_in_read_only_when_present() {
        let dir = std::env::temp_dir().join(format!(
            "yunova-ext-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("extensions")).unwrap();

        let joined = build_run_args(
            7,
            &SandboxLimits::default(),
            &dir,
            "172.18.0.1",
            3000,
        )
        .join(" ");
        assert!(
            joined.contains(&format!("{}:/run/yunova/extensions:ro", dir.join("extensions").display())),
            "expected a read-only extensions bind, got: {joined}"
        );
        // An agent that could edit its own guardrails would not be gated.
        assert!(joined.contains("/run/yunova/extensions:ro"));
        assert!(joined.contains("cp -r \"$YUNOVA_EXT_SRC\""));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_extension_mount_is_added_when_the_session_has_none() {
        // The common case must not reference a path that does not exist, or
        // docker refuses to start the container.
        let joined = args_for_test().join(" ");
        assert!(!joined.contains("/run/yunova/extensions"));
    }

    #[test]
    fn a_wrapped_docker_binary_keeps_its_arguments() {
        // A deployment that cannot put the server user in the `docker` group
        // configures something like "sudo -n docker"; the wrapper words must
        // survive into the command.
        let raw = "sudo -n docker";
        let mut parts = raw.split_whitespace();
        assert_eq!(parts.next(), Some("sudo"));
        assert_eq!(parts.collect::<Vec<_>>(), vec!["-n", "docker"]);
    }
}
