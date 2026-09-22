//! Global event bus → SSE at `GET /api/events`.

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<String>,
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Self { tx }
    }

    pub fn send(&self, json: String) {
        let _ = self.tx.send(json); // no receivers is fine
    }

    pub fn emit(&self, event_type: &str, case_id: &str) {
        let payload = serde_json::json!({"type": event_type, "case_id": case_id});
        self.send(payload.to_string());
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

/// SSE handler body: bus events; keep-alive pings come from `KeepAlive`.
pub fn sse_stream(
    rx: broadcast::Receiver<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = BroadcastStream::new(rx)
        .filter_map(|msg| msg.ok().map(|s| Ok(Event::default().data(s))));
    Sse::new(events).keep_alive(KeepAlive::default())
}
