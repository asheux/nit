use std::path::Path;
use std::process::Command;

use super::CodexRunnerConfig;

pub(super) fn codex_exec_endpoint_label(agent_id: &str, resume_thread_id: Option<&str>) -> String {
    let model_slug = codex_model_slug_for_agent_id(agent_id);
    let suffix = if model_slug == agent_id {
        String::new()
    } else {
        format!(" (agent {agent_id})")
    };
    if let Some(thread_id) = resume_thread_id {
        format!(
            "codex exec resume {} -m {model_slug}{suffix}",
            shorten_thread_id(thread_id),
        )
    } else {
        format!("codex exec -m {model_slug}{suffix}")
    }
}

// Strip every clone-style suffix in one go: split on the first '#'. Known
// suffix conventions (`#swarm-…`, `#chat-clone-…`, `#shadow-…`, `#mp-pane-…`)
// all start with `#`, and base model slugs never contain `#`. Handles nested
// suffixes like `claude-opus-4-7#mp-pane-01#swarm-mis-001-clone-01` — the
// FIRST `#` separates the model slug from the lane decoration.
pub(crate) fn codex_model_slug_for_agent_id(agent_id: &str) -> &str {
    match agent_id.split_once('#') {
        Some((base, _)) if !base.trim().is_empty() => base,
        _ => agent_id,
    }
}

pub(super) fn build_codex_mcp_tool_call(
    agent_id: &str,
    prompt: &str,
    cwd: &Path,
    reasoning_effort: Option<&str>,
    config: &CodexRunnerConfig,
    resume_thread_id: Option<&str>,
    read_only: bool,
) -> (&'static str, serde_json::Value) {
    if let Some(thread_id) = resume_thread_id {
        return (
            "codex-reply",
            serde_json::json!({ "threadId": thread_id, "prompt": prompt }),
        );
    }

    let mut args = serde_json::Map::new();
    args.insert(
        "prompt".into(),
        serde_json::Value::String(prompt.to_string()),
    );
    args.insert(
        "model".into(),
        serde_json::Value::String(codex_model_slug_for_agent_id(agent_id).to_string()),
    );
    args.insert(
        "cwd".into(),
        serde_json::Value::String(cwd.to_string_lossy().to_string()),
    );
    if let Some(effort) = reasoning_effort.map(str::trim).filter(|s| !s.is_empty()) {
        args.insert(
            "config".into(),
            serde_json::json!({ "model_reasoning_effort": effort }),
        );
    }
    let sandbox_override = if read_only { Some("read-only") } else { None };
    let sandbox_value = sandbox_override.or_else(|| {
        config
            .sandbox
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });
    if let Some(sandbox) = sandbox_value {
        args.insert(
            "sandbox".into(),
            serde_json::Value::String(sandbox.to_string()),
        );
    }
    if let Some(policy) = config
        .approval_policy
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.insert(
            "approval-policy".into(),
            serde_json::Value::String(policy.to_string()),
        );
    }
    ("codex", serde_json::Value::Object(args))
}

// Spawn-site cwd binding for the codex subprocess. `-C <cwd>` is dropped on
// resume turns; `current_dir` is the only consistent cwd channel across fresh
// and resumed dispatches.
pub(super) fn prepare_codex_command(cwd: &Path, args: Vec<String>) -> Command {
    let mut cmd = Command::new("codex");
    cmd.current_dir(cwd).args(args);
    cmd
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_codex_exec_args(
    agent_id: &str,
    cwd: &Path,
    persist_session: bool,
    reasoning_effort: Option<&str>,
    out_file: &Path,
    resume_thread_id: Option<&str>,
    read_only: bool,
    config: &CodexRunnerConfig,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(policy) = config
        .approval_policy
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.push("-a".into());
        args.push(policy.to_string());
    }
    let sandbox_override = if read_only { Some("read-only") } else { None };
    let sandbox_value = sandbox_override.or_else(|| {
        config
            .sandbox
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });
    if let Some(sandbox) = sandbox_value {
        args.push("-s".into());
        args.push(sandbox.to_string());
    }

    let model_slug = codex_model_slug_for_agent_id(agent_id);
    if let Some(thread_id) = resume_thread_id {
        args.push("exec".into());
        args.push("resume".into());
        args.push("--json".into());
        args.push("-m".into());
        args.push(model_slug.to_string());
        if let Some(effort) = reasoning_effort.map(str::trim).filter(|s| !s.is_empty()) {
            // Override any global config (e.g. `xhigh`) that some models don't support.
            args.push("-c".into());
            args.push(format!("model_reasoning_effort={effort:?}"));
        }
        // nit-mcp override (when a back-channel socket is set): register
        // `nit-mcp-server` as a Codex-discoverable tool server.
        push_nit_mcp_config_args(&mut args, config, agent_id);
        args.push("-o".into());
        args.push(out_file.to_string_lossy().to_string());
        // Positional SESSION_ID comes after options for `codex exec resume`.
        args.push(thread_id.to_string());
        args.push("-".into());
        return args;
    }

    args.push("exec".into());
    args.push("--json".into());
    args.push("--color".into());
    args.push("never".into());
    if !persist_session {
        args.push("--ephemeral".into());
    }
    args.push("-m".into());
    args.push(model_slug.to_string());
    args.push("-C".into());
    args.push(cwd.to_string_lossy().to_string());
    if let Some(effort) = reasoning_effort.map(str::trim).filter(|s| !s.is_empty()) {
        // Override any global config (e.g. `xhigh`) that some models don't support.
        args.push("-c".into());
        args.push(format!("model_reasoning_effort={effort:?}"));
    }
    push_nit_mcp_config_args(&mut args, config, agent_id);
    args.push("-o".into());
    args.push(out_file.to_string_lossy().to_string());
    args.push("-".into());
    args
}

// Push `-c mcp_servers.nit=...` overrides so the child Codex process can
// discover the back-channel MCP server via a TOML inline table. Agent id
// propagates via env so signals/claims carry the right `posted_by`.
//
// Note: Codex's exact TOML inline-table escaping for `-c` overrides hasn't
// been empirically verified — if Codex rejects the override at runtime the
// in-process nit-mcp side still works; only the Codex-discoverable tool
// bridge is affected.
pub(super) fn push_nit_mcp_config_args(
    args: &mut Vec<String>,
    config: &CodexRunnerConfig,
    agent_id: &str,
) {
    let Some(socket_path) = config
        .mcp_backchannel_socket
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let Some(bin_path) = nit_mcp_server_binary_path() else {
        return;
    };
    // Escape backslashes and double quotes so the TOML-string literals remain
    // well-formed no matter what lives in $PATH.
    let bin_esc = escape_toml_string(&bin_path);
    let sock_esc = escape_toml_string(socket_path);
    let agent_esc = escape_toml_string(agent_id);
    let value = format!(
        "{{ command = \"{bin_esc}\", args = [], env = {{ NIT_MCP_BACKCHANNEL_SOCKET = \"{sock_esc}\", NIT_MCP_AGENT_ID = \"{agent_esc}\" }} }}"
    );
    args.push("-c".into());
    args.push(format!("mcp_servers.nit={value}"));
}

// Locates `nit-mcp-server` next to the running binary. `cargo install` lays it
// down alongside `nit`; development builds put it in the same `target/debug`.
// Returns `None` when discovery fails so callers can skip the `-c` injection.
fn nit_mcp_server_binary_path() -> Option<String> {
    let self_exe = std::env::current_exe().ok()?;
    let dir = self_exe.parent()?;
    let candidate = dir.join("nit-mcp-server");
    Some(candidate.to_string_lossy().into_owned())
}

fn escape_toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

fn shorten_thread_id(thread_id: &str) -> String {
    const MAX_CHARS: usize = 8;
    let id = thread_id.trim();
    match id.char_indices().nth(MAX_CHARS) {
        Some((idx, _)) => format!("{}…", &id[..idx]),
        None => id.to_string(),
    }
}
