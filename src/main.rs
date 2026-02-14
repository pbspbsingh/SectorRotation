/// main.rs — Axum server entry point
mod compute;
mod config;
mod data;
mod handlers;

#[cfg(not(debug_assertions))]
mod static_assets;

use axum::{
    routing::{get, post},
    Router,
};
use config::Config;
use data::PriceDb;
use handlers::{AppState, SharedState};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

const DB_PATH: &str = "cache.db";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("sector_rotation=debug,tower_http=info")
        .init();

    // Load config.toml — fail fast if missing or malformed
    let config = Config::load("config.toml").unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });

    info!(
        "Config loaded: {} sectors, {} industry groups, benchmark={}",
        config.sectors.len(),
        config
            .industry_groups
            .values()
            .map(|v| v.len())
            .sum::<usize>(),
        config.benchmark
    );

    // Open (or create) SQLite cache
    let db = PriceDb::open(DB_PATH).await.unwrap_or_else(|e| {
        eprintln!("ERROR opening cache DB: {e}");
        std::process::exit(1);
    });

    // Try to restore prices from cache
    let (prices, last_updated) = match db.load_prices().await {
        Ok(Some((prices, ts))) => {
            info!(
                "Loaded {} cached tickers (last updated {})",
                prices.len(),
                ts
            );
            (prices, Some(ts))
        }
        Ok(None) => {
            info!("No cached data found — start with POST /api/refresh");
            (Default::default(), None)
        }
        Err(e) => {
            tracing::warn!("Failed to read cache: {} — starting empty", e);
            (Default::default(), None)
        }
    };

    // Shared state — config lives here for the lifetime of the server
    let state: SharedState = Arc::new(RwLock::new(AppState {
        config,
        prices,
        last_updated,
        db,
    }));

    let api_router = Router::new()
        .route("/status", get(handlers::get_status))
        .route("/universe", get(handlers::get_universe))
        .route("/rrg", get(handlers::get_rrg))
        .route("/rankings", get(handlers::get_rankings))
        .route("/zscore", get(handlers::get_zscores))
        .route("/convergence", get(handlers::get_convergence))
        .route("/detail/{ticker}", get(handlers::get_detail))
        .route("/refresh", post(handlers::post_refresh))
        .with_state(state.clone());

    let app: Router<_> = Router::new().nest("/api", api_router);

    #[cfg(debug_assertions)]
    let app = {
        info!("Running in DEBUG mode: Serving files from /static folder");
        app.fallback_service(tower_http::services::ServeDir::new("static"))
    };
    #[cfg(not(debug_assertions))]
    let app = {
        info!("Running in RELEASE mode: Serving files from binary memory");
        app.fallback(static_assets::static_handler)
    };

    let port = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);
    let socket = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&socket)
        .await
        .expect("Failed to bind port 3000");

    info!("╔══════════════════════════════════════════╗");
    info!("║   Sector Rotation Dashboard              ║");
    info!("║   http://{socket}/                   ║");
    info!("╚══════════════════════════════════════════╝");
    info!("Tip: POST /api/refresh to load market data.");

    axum::serve(listener, app).await.expect("Server error");
}
