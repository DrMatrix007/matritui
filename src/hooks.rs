use std::sync::atomic::{AtomicU64, Ordering};

use crate::ipc;

static NEXT_HOOK_ID: AtomicU64 = AtomicU64::new(0);

pub fn next_hook_id() -> String {
    format!("pane-{}", NEXT_HOOK_ID.fetch_add(1, Ordering::Relaxed))
}

pub fn write_settings_file(hook_id: &str) -> std::io::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!("mmux-hooks-{hook_id}.json"));
    let exe = std::env::current_exe()?;

    let hook_entry = serde_json::json!([{
        "matcher": "*",
        "hooks": [{
            "type": "command",
            "command": exe.to_string_lossy(),
            "args": ["hook-forward"],
        }],
    }]);

    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": hook_entry,
            "SessionEnd": hook_entry,
            "UserPromptSubmit": hook_entry,
            "PreToolUse": hook_entry,
            "PostToolUse": hook_entry,
            "SubagentStart": hook_entry,
            "SubagentStop": hook_entry,
            "Stop": hook_entry,
            "Notification": hook_entry,
            "PreCompact": hook_entry,
        },
    });

    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
    Ok(path)
}

pub fn pane_launch_extras(hook_id: &str, settings_path: &std::path::Path) -> (Vec<(String, String)>, Vec<String>) {
    let env = vec![
        ("MMUX_PANE_ID".to_string(), hook_id.to_string()),
        ("MMUX_IPC_SOCKET".to_string(), ipc::socket_name()),
    ];
    let args = vec!["--settings".to_string(), settings_path.to_string_lossy().into_owned()];
    (env, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_hook_id_generates_distinct_ids() {
        assert_ne!(next_hook_id(), next_hook_id());
    }

    #[test]
    fn write_settings_file_contains_all_hook_events() {
        let path = write_settings_file("test-write-settings-file").expect("write settings file");

        let contents = std::fs::read_to_string(&path).expect("read settings file");
        let settings: serde_json::Value = serde_json::from_str(&contents).expect("parse settings json");

        let hooks = settings.get("hooks").expect("hooks key present").as_object().expect("hooks is object");

        for key in [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "SubagentStart",
            "SubagentStop",
            "Stop",
            "Notification",
            "PreCompact",
        ] {
            assert!(hooks.contains_key(key), "missing hook event key: {key}");
        }

        let session_start_entry = &hooks["SessionStart"][0];
        assert_eq!(session_start_entry["matcher"], "*");
        let hook = &session_start_entry["hooks"][0];
        assert_eq!(hook["args"], serde_json::json!(["hook-forward"]));

        std::fs::remove_file(&path).expect("remove temp settings file");
    }

    #[test]
    fn pane_launch_extras_returns_expected_env_and_args() {
        let settings_path = std::path::PathBuf::from("/tmp/mmux-hooks-test.json");
        let (env, args) = pane_launch_extras("hook-id-123", &settings_path);

        assert_eq!(
            env,
            vec![
                ("MMUX_PANE_ID".to_string(), "hook-id-123".to_string()),
                ("MMUX_IPC_SOCKET".to_string(), ipc::socket_name()),
            ]
        );
        assert_eq!(args, vec!["--settings".to_string(), settings_path.to_string_lossy().into_owned()]);
    }
}
