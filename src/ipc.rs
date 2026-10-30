use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;

use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};

pub fn socket_name() -> String {
    format!("mmux-{}", std::process::id())
}

#[derive(Clone, Debug)]
pub struct HookEvent {
    pub pane_id: String,
    pub event: serde_json::Value,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Envelope {
    pane_id: String,
    event: serde_json::Value,
}

pub struct Server {
    receiver: mpsc::Receiver<HookEvent>,
}

impl Server {
    pub fn bind(name: &str) -> std::io::Result<Server> {
        let socket_name = name.to_ns_name::<GenericNamespaced>()?;
        let listener = ListenerOptions::new().name(socket_name).create_sync()?;
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else {
                    continue;
                };
                let sender = sender.clone();
                thread::spawn(move || {
                    // Each JSON object is written as one compact line, since compact JSON
                    // never contains a literal unescaped newline; this lets us frame
                    // messages on the wire with simple newline-delimited reads.
                    for line in BufReader::new(conn).lines() {
                        let Ok(line) = line else {
                            break;
                        };
                        if let Ok(envelope) = serde_json::from_str::<Envelope>(&line) {
                            let _ = sender.send(HookEvent {
                                pane_id: envelope.pane_id,
                                event: envelope.event,
                            });
                        }
                    }
                });
            }
        });

        Ok(Server { receiver })
    }

    pub fn try_recv(&self) -> Option<HookEvent> {
        self.receiver.try_recv().ok()
    }
}

pub fn send_event(socket_name: &str, pane_id: &str, event: serde_json::Value) -> std::io::Result<()> {
    let name = socket_name.to_ns_name::<GenericNamespaced>()?;
    let mut conn = Stream::connect(name)?;
    let envelope = Envelope {
        pane_id: pane_id.to_string(),
        event,
    };
    let mut line = serde_json::to_string(&envelope)?;
    line.push('\n');
    conn.write_all(line.as_bytes())?;
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn send_event_round_trips_to_server() {
        let socket = format!("mmux-test-ipc-roundtrip-{}", std::process::id());
        let server = Server::bind(&socket).expect("bind test socket");

        let event = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"});
        send_event(&socket, "pane-42", event.clone()).expect("send event");

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
        assert_eq!(received.pane_id, "pane-42");
        assert_eq!(received.event, event);
    }
}
