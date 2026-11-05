//! AniStock v2 server. For now: load config, migrate the game database, load the state into the
//! engine (invariants checked), report, and exit. HTTP / NPC / daily close come in later steps.

use anistock::{store, Config, Engine};

fn main() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config: {e}");
            std::process::exit(2);
        }
    };
    match store::migrate(&cfg.game_db) {
        Ok(applied) if !applied.is_empty() => println!("applied migrations: {applied:?}"),
        Ok(_) => {}
        Err(e) => {
            eprintln!("{}: {e}", cfg.game_db.display());
            std::process::exit(1);
        }
    }
    let mut engine = match Engine::open(&cfg.game_db, cfg.fees(), cfg.rules()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("refusing to start: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "loaded {}: {} accounts, {} companies",
        cfg.game_db.display(),
        engine.accounts().count(),
        engine.companies().count()
    );
    if let Err(e) = engine.flush() {
        eprintln!("persist: {e}");
        std::process::exit(1);
    }
}
