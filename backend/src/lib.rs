//! AniStock v2 game server core: configuration, storage, and the in-memory exchange engine.
//!
//! Rules and data are identical to v1 (`server/`): same SQLite schema and file, same ids, same rounding,
//! same player-facing messages. See `docs/v2-architecture.md`.

pub mod config;
pub mod daily;
pub mod engine;
pub mod exchange;
pub mod inputs;
pub mod ipo;
pub mod model;
pub mod npc;
pub mod pyfmt;
pub mod pyrandom;
pub mod valuation;
pub mod store;

// HTTP layer
pub mod actor;
pub mod api;
pub mod auth;
pub mod loops;

pub use config::{Clock, Config, Phase};
pub use engine::{Engine, NewOrder, Placed};
pub use exchange::{Fees, OrderError, Rules};
