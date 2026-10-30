use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ipc;

const MAX_EVENTS_PER_PANE: usize = 200;

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

#[derive(Default)]
pub struct EventLog {
    by_pane: HashMap<String, Vec<ipc::HookEvent>>,
}

impl EventLog {
    pub fn record(&mut self, event: ipc::HookEvent) {
        let events = self.by_pane.entry(event.pane_id.clone()).or_default();
        if events.len() >= MAX_EVENTS_PER_PANE {
            events.remove(0);
        }
        events.push(event);
    }

    pub fn events_for(&self, pane_id: &str) -> &[ipc::HookEvent] {
        self.by_pane.get(pane_id).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_event(pane_id: &str, n: usize) -> ipc::HookEvent {
        ipc::HookEvent {
            pane_id: pane_id.to_string(),
            event: serde_json::json!({ "n": n }),
        }
    }

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

    #[test]
    fn event_log_isolates_and_orders_events_per_pane() {
        let mut log = EventLog::default();

        log.record(test_event("pane-a", 0));
        log.record(test_event("pane-b", 0));
        log.record(test_event("pane-a", 1));
        log.record(test_event("pane-b", 1));
        log.record(test_event("pane-a", 2));

        let a_events = log.events_for("pane-a");
        assert_eq!(a_events.len(), 3);
        assert_eq!(a_events[0].event, serde_json::json!({ "n": 0 }));
        assert_eq!(a_events[1].event, serde_json::json!({ "n": 1 }));
        assert_eq!(a_events[2].event, serde_json::json!({ "n": 2 }));

        let b_events = log.events_for("pane-b");
        assert_eq!(b_events.len(), 2);
        assert_eq!(b_events[0].event, serde_json::json!({ "n": 0 }));
        assert_eq!(b_events[1].event, serde_json::json!({ "n": 1 }));

        assert!(log.events_for("unknown-pane").is_empty());
    }

    #[test]
    fn event_log_caps_events_per_pane_and_evicts_oldest() {
        let mut log = EventLog::default();

        for n in 0..250 {
            log.record(test_event("pane-cap", n));
        }

        let events = log.events_for("pane-cap");
        assert_eq!(events.len(), MAX_EVENTS_PER_PANE);
        assert_ne!(events[0].event, serde_json::json!({ "n": 0 }));
        assert_eq!(events[events.len() - 1].event, serde_json::json!({ "n": 249 }));
    }
}
