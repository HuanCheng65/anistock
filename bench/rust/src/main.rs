mod engine;
mod exchange;
mod log;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use crossbeam_channel::Sender;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use engine::{Config, Engine, Msg, OrderIn};
use exchange::{Exchange, OType, Side, NSYM};

const WS_QUEUE: usize = 256;

#[derive(Clone)]
struct AppState {
    engine: Sender<Msg>,
    next_client: Arc<AtomicU64>,
}

fn env_or<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(d)
}

fn main() {
    let port: u16 = env_or("BENCH_PORT", 18700);
    let npc_rate: u64 = env_or("BENCH_NPC_RATE", 300);
    let mm: u64 = env_or("BENCH_MM", 1);
    let workers: usize = env_or("TOKIO_WORKER_THREADS", 2);
    let log_path = std::env::var("BENCH_LOG").unwrap_or_else(|_| "bench-rust.log".to_string());

    let file = std::fs::File::create(&log_path).expect("open log"); // truncates
    let (log_tx, log_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
    std::thread::Builder::new()
        .name("log".into())
        .spawn(move || log::run(file, log_rx))
        .expect("spawn log thread");

    let (eng_tx, eng_rx) = crossbeam_channel::unbounded::<Msg>();
    let engine = Engine::new(Exchange::new(Some(log_tx)), Config { npc_rate, mm: mm == 1 });
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || engine.run(eng_rx))
        .expect("spawn engine thread");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers.max(1))
        .enable_all()
        .build()
        .expect("tokio runtime");

    let state = AppState {
        engine: eng_tx,
        next_client: Arc::new(AtomicU64::new(1)),
    };
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/order", post(order))
        .route("/cancel", post(cancel))
        .route("/stats", get(stats))
        .route("/ws", get(ws_upgrade))
        .with_state(state);

    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("bind");
        eprintln!(
            "anistock-bench (rust) on 127.0.0.1:{port} workers={workers} npc_rate={npc_rate} mm={mm} log={log_path}"
        );
        axum::serve(listener, app).await.expect("serve");
    });
}

fn bad(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg.into() }))).into_response()
}

fn engine_gone() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "engine stopped" }))).into_response()
}

#[derive(Deserialize)]
struct OrderReq {
    acct: i64,
    sym: i64,
    side: Side,
    #[serde(rename = "type")]
    typ: OType,
    qty: i64,
    #[serde(default)]
    px: Option<i64>,
}

#[derive(Serialize)]
struct OrderResp {
    id: u64,
    filled: i64,
    status: &'static str,
}

async fn order(State(st): State<AppState>, body: Bytes) -> Response {
    let r: OrderReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad(e.to_string()),
    };
    let (tx, rx) = oneshot::channel();
    let o = OrderIn {
        acct: r.acct,
        sym: r.sym,
        side: r.side,
        typ: r.typ,
        qty: r.qty,
        px: r.px,
    };
    if st.engine.send(Msg::Order(o, tx)).is_err() {
        return engine_gone();
    }
    match rx.await {
        Ok(Ok(p)) => Json(OrderResp {
            id: p.id,
            filled: p.filled,
            status: p.status.as_str(),
        })
        .into_response(),
        Ok(Err(e)) => bad(e),
        Err(_) => engine_gone(),
    }
}

#[derive(Deserialize)]
struct CancelReq {
    acct: i64,
    id: u64,
}

async fn cancel(State(st): State<AppState>, body: Bytes) -> Response {
    let r: CancelReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad(e.to_string()),
    };
    let (tx, rx) = oneshot::channel();
    if st.engine.send(Msg::Cancel(r.acct, r.id, tx)).is_err() {
        return engine_gone();
    }
    match rx.await {
        Ok(ok) => Json(json!({ "ok": ok })).into_response(),
        Err(_) => engine_gone(),
    }
}

async fn stats(State(st): State<AppState>) -> Response {
    let (tx, rx) = oneshot::channel();
    if st.engine.send(Msg::Stats(tx)).is_err() {
        return engine_gone();
    }
    match rx.await {
        Ok(s) => Json(s).into_response(),
        Err(_) => engine_gone(),
    }
}

async fn ws_upgrade(
    State(st): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let sym = match q.get("sym").and_then(|s| s.parse::<usize>().ok()) {
        Some(s) if s < NSYM => s,
        _ => return bad("bad sym"),
    };
    ws.on_upgrade(move |socket| ws_client(socket, sym, st))
}

async fn ws_client(socket: WebSocket, sym: usize, st: AppState) {
    let id = st.next_client.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(WS_QUEUE);
    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    if st
        .engine
        .send(Msg::WsAdd { id, sym, tx, kill: kill_tx })
        .is_err()
    {
        return;
    }
    let (mut sink, mut stream) = socket.split();
    // Reader: ignore everything the client sends; ends when the client goes away.
    let reader = async move { while let Some(Ok(_)) = stream.next().await {} };
    let writer = async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    };
    // Kill (slow-client drop) wins immediately, even if a write is blocked.
    tokio::select! {
        biased;
        _ = kill_rx => {}
        _ = reader => {}
        _ = writer => {}
    }
    // Socket halves are dropped here -> connection closed.
    let _ = st.engine.send(Msg::WsDel(id));
}
