use std::collections::HashMap;

use crate::ipc;

pub enum PaneStatus {
    Idle,
    RunningTool(String),
    RunningSubagent(String),
}

pub struct EditEntry {
    pub file_path: String,
    pub before: Option<String>,
    pub after: String,
    pub added: usize,
    pub removed: usize,
}

pub struct SubagentNode {
    pub agent_id: String,
    pub agent_type: String,
    pub description: Option<String>,
    pub active: bool,
    pub current_tool: Option<String>,
    pub children: Vec<SubagentNode>,
}

struct PendingCall {
    tool_name: String,
    caller_agent_id: Option<String>,
    subagent_type: Option<String>,
    description: Option<String>,
    seq: u64,
}

#[derive(Default)]
pub struct PaneState {
    pub root_subagents: Vec<SubagentNode>,
    pub edit_timeline: Vec<EditEntry>,
    pending_calls: HashMap<String, PendingCall>,
    next_seq: u64,
}

const EDIT_TIMELINE_CAP: usize = 200;

static DEFAULT_PANE_STATE: std::sync::LazyLock<PaneState> = std::sync::LazyLock::new(PaneState::default);

#[derive(Default)]
pub struct StateEngine {
    by_pane: HashMap<String, PaneState>,
}

fn str_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|v| v.as_str())
}

fn find_node_mut<'a>(nodes: &'a mut [SubagentNode], agent_id: &str) -> Option<&'a mut SubagentNode> {
    for node in nodes {
        if node.agent_id == agent_id {
            return Some(node);
        }
        if let Some(found) = find_node_mut(&mut node.children, agent_id) {
            return Some(found);
        }
    }
    None
}

fn remove_node(nodes: &mut Vec<SubagentNode>, agent_id: &str) -> Option<SubagentNode> {
    if let Some(pos) = nodes.iter().position(|n| n.agent_id == agent_id) {
        return Some(nodes.remove(pos));
    }
    for node in nodes.iter_mut() {
        if let Some(found) = remove_node(&mut node.children, agent_id) {
            return Some(found);
        }
    }
    None
}

fn diff_stats(before: &str, after: &str) -> (usize, usize) {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

fn edit_entry_for(tool_name: &str, tool_input: &serde_json::Value) -> Option<EditEntry> {
    match tool_name {
        "Edit" => {
            let file_path = str_field(tool_input, "file_path")?.to_string();
            let old_string = str_field(tool_input, "old_string")?.to_string();
            let new_string = str_field(tool_input, "new_string")?.to_string();
            let (added, removed) = diff_stats(&old_string, &new_string);
            Some(EditEntry {
                file_path,
                before: Some(old_string),
                after: new_string,
                added,
                removed,
            })
        }
        "Write" => {
            let file_path = str_field(tool_input, "file_path")?.to_string();
            let content = str_field(tool_input, "content")?.to_string();
            let (added, removed) = diff_stats("", &content);
            Some(EditEntry {
                file_path,
                before: None,
                after: content,
                added,
                removed,
            })
        }
        "NotebookEdit" => {
            let file_path = str_field(tool_input, "file_path")?.to_string();
            let new_source = str_field(tool_input, "new_source")?.to_string();
            let (added, removed) = diff_stats("", &new_source);
            Some(EditEntry {
                file_path,
                before: None,
                after: new_source,
                added,
                removed,
            })
        }
        _ => None,
    }
}

impl PaneState {
    pub fn status(&self) -> PaneStatus {
        let top_level_call = self
            .pending_calls
            .values()
            .filter(|call| call.caller_agent_id.is_none())
            .min_by_key(|call| call.seq);
        if let Some(call) = top_level_call {
            return PaneStatus::RunningTool(call.tool_name.clone());
        }

        if let Some(node) = self.root_subagents.iter().find_map(|node| active_node(node)) {
            return PaneStatus::RunningSubagent(node.agent_type.clone());
        }

        PaneStatus::Idle
    }

    fn push_edit(&mut self, entry: EditEntry) {
        if self.edit_timeline.len() >= EDIT_TIMELINE_CAP {
            self.edit_timeline.remove(0);
        }
        self.edit_timeline.push(entry);
    }
}

fn active_node(node: &SubagentNode) -> Option<&SubagentNode> {
    if node.active {
        return Some(node);
    }
    node.children.iter().find_map(active_node)
}

impl StateEngine {
    pub fn state_for(&self, pane_id: &str) -> &PaneState {
        self.by_pane.get(pane_id).unwrap_or(&DEFAULT_PANE_STATE)
    }

    pub fn record(&mut self, event: &ipc::HookEvent) {
        let Some(hook_event_name) = str_field(&event.event, "hook_event_name") else {
            return;
        };

        match hook_event_name {
            "PreToolUse" => self.record_pre_tool_use(event),
            "PostToolUse" => self.record_post_tool_use(event),
            "SubagentStart" => self.record_subagent_start(event),
            "SubagentStop" => self.record_subagent_stop(event),
            _ => {}
        }
    }

    fn record_pre_tool_use(&mut self, event: &ipc::HookEvent) {
        let Some(tool_use_id) = str_field(&event.event, "tool_use_id") else {
            return;
        };
        let Some(tool_name) = str_field(&event.event, "tool_name") else {
            return;
        };
        let tool_name = tool_name.to_string();
        let caller_agent_id = str_field(&event.event, "agent_id").map(String::from);

        let pane = self.by_pane.entry(event.pane_id.clone()).or_default();

        let (subagent_type, description) = if tool_name == "Agent" || tool_name == "Task" {
            let tool_input = event.event.get("tool_input");
            let subagent_type = tool_input.and_then(|v| str_field(v, "subagent_type")).map(String::from);
            let description = tool_input.and_then(|v| str_field(v, "description")).map(String::from);
            (subagent_type, description)
        } else {
            (None, None)
        };

        let seq = pane.next_seq;
        pane.next_seq += 1;
        pane.pending_calls.insert(
            tool_use_id.to_string(),
            PendingCall {
                tool_name: tool_name.clone(),
                caller_agent_id,
                subagent_type,
                description,
                seq,
            },
        );

        if matches!(tool_name.as_str(), "Edit" | "Write" | "NotebookEdit")
            && let Some(tool_input) = event.event.get("tool_input")
            && let Some(entry) = edit_entry_for(&tool_name, tool_input)
        {
            pane.push_edit(entry);
        }

        if let Some(agent_id) = str_field(&event.event, "agent_id") {
            if let Some(node) = find_node_mut(&mut pane.root_subagents, agent_id) {
                node.current_tool = Some(tool_name);
            } else {
                let agent_type = str_field(&event.event, "agent_type").unwrap_or("unknown").to_string();
                pane.root_subagents.push(SubagentNode {
                    agent_id: agent_id.to_string(),
                    agent_type,
                    description: None,
                    active: true,
                    current_tool: Some(tool_name),
                    children: Vec::new(),
                });
            }
        }
    }

    fn record_post_tool_use(&mut self, event: &ipc::HookEvent) {
        let pane = self.by_pane.entry(event.pane_id.clone()).or_default();

        if let Some(tool_use_id) = str_field(&event.event, "tool_use_id") {
            pane.pending_calls.remove(tool_use_id);
        }

        if let Some(agent_id) = str_field(&event.event, "agent_id")
            && let Some(node) = find_node_mut(&mut pane.root_subagents, agent_id)
        {
            node.current_tool = None;
        }
    }

    fn record_subagent_start(&mut self, event: &ipc::HookEvent) {
        let Some(agent_id) = str_field(&event.event, "agent_id") else {
            return;
        };
        let Some(agent_type) = str_field(&event.event, "agent_type") else {
            return;
        };
        let agent_id = agent_id.to_string();
        let agent_type = agent_type.to_string();

        let pane = self.by_pane.entry(event.pane_id.clone()).or_default();

        let matched_key = pane
            .pending_calls
            .iter()
            .filter(|(_, call)| {
                matches!(call.tool_name.as_str(), "Agent" | "Task") && call.subagent_type.as_deref() == Some(agent_type.as_str())
            })
            .min_by_key(|(_, call)| call.seq)
            .map(|(key, _)| key.clone());

        let (description, caller_agent_id) = match matched_key {
            Some(key) => {
                let call = pane.pending_calls.remove(&key).expect("matched key exists");
                (call.description, call.caller_agent_id)
            }
            None => (None, None),
        };

        let node = match remove_node(&mut pane.root_subagents, &agent_id) {
            Some(mut existing) => {
                existing.agent_type = agent_type;
                if let Some(description) = description {
                    existing.description = Some(description);
                }
                existing.active = true;
                existing
            }
            None => SubagentNode {
                agent_id,
                agent_type,
                description,
                active: true,
                current_tool: None,
                children: Vec::new(),
            },
        };

        match caller_agent_id.and_then(|parent_id| find_node_mut(&mut pane.root_subagents, &parent_id)) {
            Some(parent) => parent.children.push(node),
            None => pane.root_subagents.push(node),
        }
    }

    fn record_subagent_stop(&mut self, event: &ipc::HookEvent) {
        let Some(agent_id) = str_field(&event.event, "agent_id") else {
            return;
        };

        let pane = self.by_pane.entry(event.pane_id.clone()).or_default();
        if let Some(node) = find_node_mut(&mut pane.root_subagents, agent_id) {
            node.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(pane_id: &str, json: serde_json::Value) -> ipc::HookEvent {
        ipc::HookEvent {
            pane_id: pane_id.to_string(),
            event: json,
        }
    }

    #[test]
    fn idle_by_default() {
        let engine = StateEngine::default();
        let state = engine.state_for("unknown-pane");

        assert!(matches!(state.status(), PaneStatus::Idle));
        assert!(state.root_subagents.is_empty());
        assert!(state.edit_timeline.is_empty());
    }

    #[test]
    fn pre_tool_use_without_post_marks_tool_running() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_use_id": "t1"
            }),
        ));

        let status = engine.state_for("pane1").status();
        assert!(matches!(status, PaneStatus::RunningTool(ref name) if name == "Bash"));
    }

    #[test]
    fn matching_post_tool_use_returns_to_idle() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_use_id": "t1"
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_use_id": "t1"
            }),
        ));

        assert!(matches!(engine.state_for("pane1").status(), PaneStatus::Idle));
    }

    #[test]
    fn subagent_start_creates_node_with_description_and_running_status() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "t1",
                "tool_input": {"subagent_type": "Explore", "description": "find bugs"}
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));

        let state = engine.state_for("pane1");
        match state.status() {
            PaneStatus::RunningSubagent(label) => assert!(label.contains("Explore")),
            _ => panic!("expected RunningSubagent status"),
        }
        assert_eq!(state.root_subagents.len(), 1);
        let node = &state.root_subagents[0];
        assert_eq!(node.agent_type, "Explore");
        assert_eq!(node.description, Some("find bugs".to_string()));
    }

    #[test]
    fn subagent_stop_returns_to_idle_but_keeps_node_in_tree() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "t1",
                "tool_input": {"subagent_type": "Explore", "description": "find bugs"}
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStop",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));

        let state = engine.state_for("pane1");
        assert!(matches!(state.status(), PaneStatus::Idle));
        assert_eq!(state.root_subagents.len(), 1);
        assert!(!state.root_subagents[0].active);
    }

    #[test]
    fn nested_tool_call_attaches_to_subagent_node_not_pane_status() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "t1",
                "tool_input": {"subagent_type": "Explore", "description": "find bugs"}
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_use_id": "t2",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));

        let state = engine.state_for("pane1");
        assert!(matches!(state.status(), PaneStatus::RunningSubagent(_)));
        assert_eq!(state.root_subagents[0].current_tool, Some("Bash".to_string()));
    }

    #[test]
    fn two_sequential_same_type_subagents_matched_fifo_not_swapped() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "t1",
                "tool_input": {"subagent_type": "Explore", "description": "find X"}
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "t2",
                "tool_input": {"subagent_type": "Explore", "description": "find Y"}
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a2",
                "agent_type": "Explore"
            }),
        ));

        let state = engine.state_for("pane1");
        let node_a1 = state.root_subagents.iter().find(|n| n.agent_id == "a1").unwrap();
        let node_a2 = state.root_subagents.iter().find(|n| n.agent_id == "a2").unwrap();
        assert_eq!(node_a1.description, Some("find X".to_string()));
        assert_eq!(node_a2.description, Some("find Y".to_string()));
    }

    #[test]
    fn edit_tool_creates_timeline_entry_with_before_after_and_diff_stat() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Edit",
                "tool_use_id": "t1",
                "tool_input": {
                    "file_path": "/tmp/foo.rs",
                    "old_string": "a\nb\n",
                    "new_string": "a\nc\nd\n"
                }
            }),
        ));

        let state = engine.state_for("pane1");
        assert_eq!(state.edit_timeline.len(), 1);
        let entry = &state.edit_timeline[0];
        assert_eq!(entry.file_path, "/tmp/foo.rs");
        assert_eq!(entry.before, Some("a\nb\n".to_string()));
        assert_eq!(entry.after, "a\nc\nd\n".to_string());
        assert_eq!(entry.added, 2);
        assert_eq!(entry.removed, 1);
    }

    #[test]
    fn write_tool_with_no_prior_content_records_all_added_no_removed() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Write",
                "tool_use_id": "t1",
                "tool_input": {"file_path": "/tmp/new.rs", "content": "x\ny\n"}
            }),
        ));

        let state = engine.state_for("pane1");
        assert_eq!(state.edit_timeline.len(), 1);
        let entry = &state.edit_timeline[0];
        assert_eq!(entry.before, None);
        assert_eq!(entry.after, "x\ny\n".to_string());
        assert_eq!(entry.removed, 0);
        assert_eq!(entry.added, 2);
    }

    #[test]
    fn malformed_or_unrelated_events_are_ignored_without_panic() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({"hook_event_name": "SomeUnknownHook"}),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Edit",
                "tool_use_id": "t9"
            }),
        ));

        let state = engine.state_for("pane1");
        assert!(state.edit_timeline.is_empty());
        assert!(state.root_subagents.is_empty());
        assert!(matches!(state.status(), PaneStatus::RunningTool(ref name) if name == "Edit"));
    }

    #[test]
    fn subagent_start_updates_existing_placeholder_node_instead_of_duplicating() {
        let mut engine = StateEngine::default();
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_use_id": "t1",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));

        {
            let state = engine.state_for("pane1");
            assert_eq!(state.root_subagents.len(), 1);
            assert_eq!(state.root_subagents[0].agent_id, "a1");
        }

        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));

        {
            let state = engine.state_for("pane1");
            assert_eq!(state.root_subagents.len(), 1);
            assert_eq!(state.root_subagents[0].agent_type, "Explore");
        }

        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStop",
                "agent_id": "a1"
            }),
        ));

        let state = engine.state_for("pane1");
        assert_eq!(state.root_subagents.len(), 1);
        assert!(!state.root_subagents[0].active);
    }

    #[test]
    fn subagent_start_reparents_misplaced_root_placeholder_under_real_caller() {
        let mut engine = StateEngine::default();

        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "tA",
                "tool_input": {"subagent_type": "Explore", "description": "root agent"}
            }),
        ));
        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "a1",
                "agent_type": "Explore"
            }),
        ));

        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Agent",
                "tool_use_id": "tB",
                "tool_input": {"subagent_type": "Task", "description": "nested agent"},
                "agent_id": "a1"
            }),
        ));

        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_use_id": "tBash",
                "agent_id": "b1",
                "agent_type": "Task"
            }),
        ));

        engine.record(&event(
            "pane1",
            serde_json::json!({
                "hook_event_name": "SubagentStart",
                "agent_id": "b1",
                "agent_type": "Task"
            }),
        ));

        let state = engine.state_for("pane1");
        assert_eq!(state.root_subagents.len(), 1);
        assert!(state.root_subagents.iter().find(|n| n.agent_id == "b1").is_none());
        assert_eq!(state.root_subagents[0].agent_id, "a1");
        assert!(state.root_subagents[0].children.iter().any(|n| n.agent_id == "b1"));
    }
}
