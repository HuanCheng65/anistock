//! `GET /api/v1/ws`: the real-time push (docs/api-v1.md §5). What is sent and when is `live.rs`; this
//! file is the connection: authentication at the handshake, the client messages, the writer, close codes.
//!
//! Authentication: the `session` cookie (browsers; may subscribe `me`) or `Authorization: Bearer <bot
//! token>` (bots; public topics and `events`). A bad bot token is refused before the upgrade with 401 and
//! the §2.9 error JSON. Close codes: 4001 session ended (logout, expiry), 4008 too slow (queue full),
//! 4029 too many connections from one address.
//!
//! Behind nginx (the server pings every 25 s and sends `time` every 20 s):
//!     location /api/v1/ws { proxy_pass http://127.0.0.1:18620; proxy_http_version 1.1;
//!         proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade";
//!         proxy_set_header Host $host; proxy_set_header X-Real-IP $remote_addr; proxy_read_timeout 120s; }

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use utoipa::ToSchema;

use super::types::*;
use crate::api::AppState;
use crate::live::{batch, Ctl, QUEUE};

const READ_BUF: usize = 4 * 1024;
const MAX_IN: usize = 16 * 1024;
const PING_EVERY: Duration = Duration::from_secs(25);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------- messages (documented in OpenAPI)

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct WsLimits {
    pub stocks: i64,
    pub queue: i64,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct WsRejected {
    pub topic: String,
    /// unknown_topic、unauthenticated、forbidden、too_many_stocks、stock_not_found
    pub code: String,
}

/// 服务器→客户端的消息（NDJSON：一帧一条或多条，以换行分隔）
#[derive(Serialize, ToSchema, Clone, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum WsServerMessage {
    Hello {
        v: i64,
        server_version: String,
        me: bool,
        bot: bool,
        now: Timestamp,
        trading_day: TradingDay,
        phase: Phase,
        limits: WsLimits,
    },
    Subscribed {
        topics: Vec<String>,
        rejected: Vec<WsRejected>,
    },
    Pong {
        #[schema(value_type = i64)]
        ping: Value,
    },
    Time {
        now: Timestamp,
        trading_day: TradingDay,
        phase: Phase,
        phase_ends_at: Option<Timestamp>,
        last_closed_day: Option<TradingDay>,
    },
    Reset {
        topic: String,
    },
    /// 行：`[id, price, prev_close, volume, turnover, vcm_active]`（列见 `cols`）
    Quotes {
        seq: u64,
        full: bool,
        trading_day: TradingDay,
        cols: Option<Vec<String>>,
        #[schema(value_type = Vec<Vec<Object>>)]
        rows: Vec<Value>,
    },
    /// 档位 `[price, quantity, orders]`，成交 `[id, price, quantity, at]`
    Stock {
        seq: u64,
        id: i64,
        full: bool,
        trading_day: TradingDay,
        as_of: Timestamp,
        price: Money,
        prev_close: Money,
        open: Option<Money>,
        high: Option<Money>,
        low: Option<Money>,
        volume: i64,
        turnover: Money,
        open_reference: Option<Money>,
        #[schema(value_type = Vec<Vec<i64>>)]
        bids: Vec<Value>,
        #[schema(value_type = Vec<Vec<i64>>)]
        asks: Vec<Value>,
        vcm: Vcm,
        auction: Option<AuctionIndication>,
        #[schema(value_type = Vec<Vec<Object>>)]
        trades: Vec<Value>,
    },
    News {
        seq: u64,
        item: NewsItem,
    },
    Order {
        seq: u64,
        order: Order,
    },
    Fill {
        seq: u64,
        trade: MyTrade,
    },
    Cash {
        seq: u64,
        cash: Money,
        frozen_cash: Money,
        available_cash: Money,
    },
    Event(super::events::Event),
}

/// 客户端→服务器的消息（每帧一个 JSON 对象，最大 16 KB）
#[derive(Deserialize, Serialize, ToSchema, Default, Debug)]
pub struct WsClientMessage {
    #[serde(default)]
    pub sub: Vec<String>,
    #[serde(default)]
    pub unsub: Vec<String>,
    /// 断线重连时各主题最后见到的 seq：`{"news": 8890, "events": 10233}`
    #[serde(default)]
    pub since: HashMap<String, i64>,
    #[serde(default)]
    pub resync: Vec<String>,
    #[schema(value_type = Option<i64>)]
    pub ping: Option<Value>,
}

// ---------------------------------------------------------------- connections per address

fn per_ip() -> &'static Mutex<HashMap<String, usize>> {
    static M: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

struct IpSlot(String);
impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut m = per_ip().lock().unwrap();
        if let Some(n) = m.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.0);
            }
        }
    }
}

/// The `exp` of a session cookie (unix seconds), for the expiry check.
fn cookie_exp(headers: &HeaderMap) -> Option<i64> {
    let token = headers
        .get_all("cookie")
        .iter()
        .find_map(|v| crate::auth::cookie_value(&[&String::from_utf8_lossy(v.as_bytes())], "session"))?;
    token.split('.').nth(1)?.parse().ok()
}

pub async fn handler(
    State(st): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let fail = |e: super::E| {
        let mut r = e.into_response();
        if let Ok(v) = axum::http::HeaderValue::from_str(super::VERSION) {
            r.headers_mut().insert("aniex-version", v);
        }
        r
    };
    let Some(live) = st.live.clone() else { return fail(super::error::unavailable()) };
    let bot = match super::bearer(&st, &headers) {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    let account = if bot.is_some() { None } else { super::session_account(&st, &headers) };
    let exp = account.and_then(|_| cookie_exp(&headers));
    let ip = super::client_ip(Some(peer), &headers);
    let over = {
        let mut m = per_ip().lock().unwrap();
        let n = m.entry(ip.clone()).or_default();
        if *n >= st.v1.limits.ws_per_ip {
            true
        } else {
            *n += 1;
            false
        }
    };
    let up = ws.read_buffer_size(READ_BUF).write_buffer_size(0).max_message_size(MAX_IN).max_frame_size(MAX_IN);
    if over {
        return up
            .on_upgrade(|mut socket| async move {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame { code: 4029, reason: "too many connections".into() })))
                    .await;
            })
            .into_response();
    }
    let slot = IpSlot(ip);
    up.on_upgrade(move |socket| client(socket, account, bot.is_some(), exp, live, slot)).into_response()
}

async fn client(
    socket: WebSocket,
    account: Option<i64>,
    bot: bool,
    exp: Option<i64>,
    live: crossbeam_channel::Sender<Ctl>,
    _slot: IpSlot,
) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let (kill_tx, kill_rx) = oneshot::channel::<u16>();
    if live.send(Ctl::Add { id, account, bot, exp, tx, kill: kill_tx }).is_err() {
        return;
    }
    let (sink, mut stream) = socket.split();
    let sink = Arc::new(tokio::sync::Mutex::new(sink));
    let live2 = live.clone();
    let reader = async move {
        while let Some(Ok(m)) = stream.next().await {
            let text = match m {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(msg) = serde_json::from_str::<WsClientMessage>(text.as_str()) else { continue };
            let clip = |v: Vec<String>| v.into_iter().take(32).filter(|t| t.len() <= 40).collect::<Vec<_>>();
            if let Some(p) = msg.ping {
                if live2.send(Ctl::Ping { id, ping: p }).is_err() {
                    break;
                }
            }
            if !msg.sub.is_empty() && live2.send(Ctl::Sub { id, topics: clip(msg.sub), since: msg.since }).is_err() {
                break;
            }
            if !msg.unsub.is_empty() && live2.send(Ctl::Unsub { id, topics: clip(msg.unsub) }).is_err() {
                break;
            }
            if !msg.resync.is_empty() && live2.send(Ctl::Resync { id, topics: clip(msg.resync) }).is_err() {
                break;
            }
        }
    };
    let sink2 = sink.clone();
    let writer = async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let Some(first) = m else { break };
                    if sink2.lock().await.send(Message::Text(batch(first, &mut rx))).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if sink2.lock().await.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    let code = tokio::select! {
        biased;
        c = kill_rx => c.ok(),
        _ = reader => None,
        _ = writer => None,
    };
    if let Some(code) = code {
        let reason = match code {
            4001 => "session ended",
            4008 => "too slow",
            _ => "closing",
        };
        let mut s = sink.lock().await;
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            s.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))),
        )
        .await;
    }
    let _ = live.send(Ctl::Del(id));
}
