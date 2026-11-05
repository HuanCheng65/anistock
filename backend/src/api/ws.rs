//! `GET /api/ws`: the real-time push channel (see `live.rs` for what is sent and when).
//!
//! The client subscribes with `{"sub":["quotes","stock:103303","news","me"]}` and leaves a topic with
//! `{"unsub":[...]}`. The session cookie is optional: anonymous connections get market data, a signed-in
//! player also gets `me` (own orders, fills, cash). Server frames are NDJSON: one or more JSON messages
//! separated by newlines.
//!
//! Behind nginx, the upgrade needs (the server pings every 25 s, so the default 60 s read timeout holds):
//!     location /api/ws { proxy_pass http://127.0.0.1:18620; proxy_http_version 1.1;
//!         proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade";
//!         proxy_set_header Host $host; proxy_read_timeout 120s; }

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};

use super::AppState;
use crate::auth::{cookie_value, read_session, Session};
use crate::live::{batch, Ctl, QUEUE};

// small per-connection buffers (spec §4): clients only send short subscribe messages
const READ_BUF: usize = 4 * 1024;
const MAX_IN: usize = 16 * 1024;
/// keeps idle connections open through proxies (nginx closes after 60 s of silence by default)
const PING_EVERY: Duration = Duration::from_secs(25);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub async fn handler(st: Arc<AppState>, ws: WebSocketUpgrade, headers: HeaderMap) -> Response {
    let Some(live) = st.live.clone() else {
        return super::detail(404, "没有这个接口");
    };
    let cookie = headers.get("cookie").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    let token = cookie.and_then(|h| cookie_value(&[h.as_str()], "session"));
    let account = match read_session(&st.secret, token.as_deref(), st.unix_now()) {
        Session::Player(a) => Some(a),
        _ => None,
    };
    ws.read_buffer_size(READ_BUF)
        .write_buffer_size(0)
        .max_message_size(MAX_IN)
        .max_frame_size(MAX_IN)
        .on_upgrade(move |socket| client(socket, account, live))
        .into_response()
}

#[derive(serde::Deserialize, Default)]
struct In {
    #[serde(default)]
    sub: Vec<String>,
    #[serde(default)]
    unsub: Vec<String>,
}

async fn client(socket: WebSocket, account: Option<i64>, live: crossbeam_channel::Sender<Ctl>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    if live.send(Ctl::Add { id, account, tx, kill: kill_tx }).is_err() {
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let live2 = live.clone();
    let reader = async move {
        while let Some(Ok(m)) = stream.next().await {
            let text = match m {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(msg) = serde_json::from_str::<In>(text.as_str()) else { continue };
            let clip = |v: Vec<String>| v.into_iter().take(32).filter(|t| t.len() <= 40).collect::<Vec<_>>();
            if !msg.sub.is_empty() && live2.send(Ctl::Sub { id, topics: clip(msg.sub) }).is_err() {
                break;
            }
            if !msg.unsub.is_empty() && live2.send(Ctl::Unsub { id, topics: clip(msg.unsub) }).is_err() {
                break;
            }
        }
    };
    let writer = async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let Some(first) = m else { break };
                    if sink.send(Message::Text(batch(first, &mut rx))).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    // a dropped (slow) client ends at once, even inside a blocked write
    tokio::select! {
        biased;
        _ = kill_rx => {}
        _ = reader => {}
        _ = writer => {}
    }
    let _ = live.send(Ctl::Del(id));
}
