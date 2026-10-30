use crate::ipc;

pub fn forward(pane_id: Option<String>, socket_name: Option<String>, stdin: &[u8]) {
    let Some(pane_id) = pane_id else {
        return;
    };
    let Some(socket_name) = socket_name else {
        return;
    };
    let Ok(event) = serde_json::from_slice(stdin) else {
        return;
    };
    let _ = ipc::send_event(&socket_name, &pane_id, event);
}

pub fn run() {
    use std::io::Read;

    let mut buf = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut buf);

    let pane_id = std::env::var("MMUX_PANE_ID").ok();
    let socket_name = std::env::var("MMUX_IPC_SOCKET").ok();

    forward(pane_id, socket_name, &buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn forward_sends_event_to_server() {
        let socket = format!("mmux-test-hookfwd-happy-{}", std::process::id());
        let server = ipc::Server::bind(&socket).expect("bind test socket");

        let payload = serde_json::json!({"hook_event_name": "Stop"});
        let stdin = serde_json::to_vec(&payload).expect("serialize payload");

        forward(Some("pane-x".to_string()), Some(socket.clone()), &stdin);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut received = None;
        while Instant::now() < deadline {
            if let Some(ev) = server.try_recv() {
                received = Some(ev);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let received = received.expect("event should have arrived within timeout");
        assert_eq!(received.pane_id, "pane-x");
        assert_eq!(received.event, payload);
    }

    #[test]
    fn forward_with_missing_pane_id_sends_nothing() {
        let socket = format!("mmux-test-hookfwd-nopane-{}", std::process::id());
        let server = ipc::Server::bind(&socket).expect("bind test socket");

        let payload = serde_json::json!({"hook_event_name": "Stop"});
        let stdin = serde_json::to_vec(&payload).expect("serialize payload");

        forward(None, Some(socket.clone()), &stdin);

        std::thread::sleep(Duration::from_millis(200));
        assert!(server.try_recv().is_none());
    }

    #[test]
    fn forward_with_malformed_json_sends_nothing() {
        let socket = format!("mmux-test-hookfwd-badjson-{}", std::process::id());
        let server = ipc::Server::bind(&socket).expect("bind test socket");

        forward(Some("pane-y".to_string()), Some(socket.clone()), b"not json");

        std::thread::sleep(Duration::from_millis(200));
        assert!(server.try_recv().is_none());
    }
}
