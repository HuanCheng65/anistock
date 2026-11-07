//! AniStock v2 game server core: configuration, storage, and the in-memory exchange engine.
//!
//! The exchange rules started as a port of v1 (`server/`) and are still checked against it by the engine
//! and close difftests (`backend/difftest/`). The HTTP API is API v1 (docs/api-v1.md). See
//! `docs/v2-architecture.md`.

pub mod config;
pub mod corporate;
pub mod daily;
pub mod futures;
pub mod engine;
pub mod exchange;
pub mod inputs;
pub mod market;
pub mod matching;
pub mod ipo;
pub mod margin;
pub mod model;
pub mod npc;
pub mod pyfmt;
pub mod pyrandom;
pub mod restructure;
pub mod sim;
pub mod valuation;
pub mod vclose;
pub mod vfilter;
pub mod store;

// HTTP layer
pub mod actor;
pub mod api;
pub mod assessment;
pub mod kyc;
pub mod surveil;
pub mod auth;
pub mod live;
pub mod loops;

pub use config::{Clock, Config, Phase};
pub use engine::{Engine, NewOrder, Placed};
pub use exchange::{Fees, OrderError, Rules};
