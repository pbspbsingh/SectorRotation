/// handlers.rs — Axum route handlers
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{
    compute::{
        compute_convergence, compute_rankings, compute_rrg, compute_zscores, ConvergenceEntry,
        RankEntry, RrgEntry, Timeframe, ZScoreEntry, DAILY, WEEKLY,
    },
    config::Config,
    data::{align, fetch_incremental, merge_prices, resample_all, PriceDb, PriceMap},
};

// ─── Application State ────────────────────────────────────────────────────────

pub struct AppState {
    pub config: Config,
    pub prices: PriceMap,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub db: PriceDb,
}

pub type SharedState = Arc<RwLock<AppState>>;

// ─── Query params ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LayerQuery {
    /// "sector" (default) or "industry"
    layer: Option<String>,
    /// "daily" (default) or "weekly"
    timeframe: Option<String>,
}

impl LayerQuery {
    fn resolve_timeframe(&self) -> &'static Timeframe {
        match self.timeframe.as_deref() {
            Some("weekly") => &WEEKLY,
            _ => &DAILY,
        }
    }

    fn is_weekly(&self) -> bool {
        self.timeframe.as_deref() == Some("weekly")
    }
}

/// Prepare prices for the requested timeframe — daily as-is, weekly via resample.
fn prices_for_timeframe(daily: &PriceMap, query: &LayerQuery) -> PriceMap {
    if query.is_weekly() {
        resample_all(daily)
    } else {
        daily.clone()
    }
}

// ─── Response envelope ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub ok: bool,
    pub data: T,
    pub last_updated: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T, last_updated: Option<chrono::DateTime<chrono::Utc>>) -> Json<Self> {
        Json(Self {
            ok: true,
            data,
            last_updated: last_updated.map(|t| t.to_rfc3339()),
        })
    }
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: String,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

fn err(msg: impl Into<String>) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            ok: false,
            error: msg.into(),
        }),
    )
}

fn no_data() -> ApiError {
    err("No data loaded yet. Call POST /api/refresh first.")
}

// ─── Layer resolution ─────────────────────────────────────────────────────────

/// Returns the ticker/name pairs for the requested layer, borrowing from Config.
fn resolve_pairs<'a>(config: &'a Config, layer: &LayerQuery) -> Vec<(&'a str, &'a str)> {
    match layer.layer.as_deref().unwrap_or("sector") {
        "industry" => config.industry_pairs(),
        _ => config.sector_pairs(),
    }
}

// ─── GET /api/rrg ─────────────────────────────────────────────────────────────

pub async fn get_rrg(
    State(state): State<SharedState>,
    Query(query): Query<LayerQuery>,
) -> Result<Json<ApiResponse<Vec<RrgEntry>>>, ApiError> {
    let state = state.read().await;
    if state.prices.is_empty() {
        return Err(no_data());
    }

    let prices = prices_for_timeframe(&state.prices, &query);
    let tickers = resolve_pairs(&state.config, &query);
    let tf = query.resolve_timeframe();
    let entries = compute_rrg(&prices, &state.config.benchmark, &tickers, tf);
    Ok(ApiResponse::ok(entries, state.last_updated))
}

// ─── GET /api/rankings ────────────────────────────────────────────────────────

pub async fn get_rankings(
    State(state): State<SharedState>,
    Query(query): Query<LayerQuery>,
) -> Result<Json<ApiResponse<Vec<RankEntry>>>, ApiError> {
    let state = state.read().await;
    if state.prices.is_empty() {
        return Err(no_data());
    }

    let prices = prices_for_timeframe(&state.prices, &query);
    let tickers = resolve_pairs(&state.config, &query);
    let tf = query.resolve_timeframe();
    let mut entries = compute_rankings(&prices, &state.config.benchmark, &tickers, tf);
    entries.sort_by_key(|e| e.rank_20d);
    Ok(ApiResponse::ok(entries, state.last_updated))
}

// ─── GET /api/zscore ──────────────────────────────────────────────────────────

pub async fn get_zscores(
    State(state): State<SharedState>,
    Query(query): Query<LayerQuery>,
) -> Result<Json<ApiResponse<Vec<ZScoreEntry>>>, ApiError> {
    let state = state.read().await;
    if state.prices.is_empty() {
        return Err(no_data());
    }

    let prices = prices_for_timeframe(&state.prices, &query);
    let tickers = resolve_pairs(&state.config, &query);
    let tf = query.resolve_timeframe();
    let mut entries = compute_zscores(&prices, &state.config.benchmark, &tickers, tf);
    entries.sort_by(|a, b| {
        b.z_short
            .partial_cmp(&a.z_short)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(ApiResponse::ok(entries, state.last_updated))
}

// ─── GET /api/convergence ─────────────────────────────────────────────────────

pub async fn get_convergence(
    State(state): State<SharedState>,
    Query(query): Query<LayerQuery>,
) -> Result<Json<ApiResponse<Vec<ConvergenceEntry>>>, ApiError> {
    let state = state.read().await;
    if state.prices.is_empty() {
        return Err(no_data());
    }

    let prices = prices_for_timeframe(&state.prices, &query);
    let tickers = resolve_pairs(&state.config, &query);
    let tf = query.resolve_timeframe();
    let rrg = compute_rrg(&prices, &state.config.benchmark, &tickers, tf);
    let rankings = compute_rankings(&prices, &state.config.benchmark, &tickers, tf);
    let zscores = compute_zscores(&prices, &state.config.benchmark, &tickers, tf);
    let mut entries = compute_convergence(&rrg, &rankings, &zscores);
    entries.sort_by_key(|e| match e.confidence.as_str() {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    });
    Ok(ApiResponse::ok(entries, state.last_updated))
}

// ─── GET /api/detail/:ticker ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DetailResponse {
    ticker: String,
    name: String,
    rrg: Option<RrgEntry>,
    rank: Option<RankEntry>,
    zscore: Option<ZScoreEntry>,
    convergence: Option<ConvergenceEntry>,
    price_history: Vec<PricePoint>,
}

#[derive(Serialize)]
pub struct PricePoint {
    date: String,
    price: f64,
    benchmark: f64,
    ratio: f64,
}

#[derive(Deserialize)]
pub struct DetailQuery {
    timeframe: Option<String>,
}

impl DetailQuery {
    fn resolve_timeframe(&self) -> &'static Timeframe {
        match self.timeframe.as_deref() {
            Some("weekly") => &WEEKLY,
            _ => &DAILY,
        }
    }

    fn is_weekly(&self) -> bool {
        self.timeframe.as_deref() == Some("weekly")
    }
}

pub async fn get_detail(
    State(state): State<SharedState>,
    Path(ticker): Path<String>,
    Query(query): Query<DetailQuery>,
) -> Result<Json<ApiResponse<DetailResponse>>, ApiError> {
    let state = state.read().await;
    if state.prices.is_empty() {
        return Err(no_data());
    }

    let prices = if query.is_weekly() {
        resample_all(&state.prices)
    } else {
        state.prices.clone()
    };
    let tf = query.resolve_timeframe();

    let cfg = &state.config;
    let name = cfg.name_of(&ticker).unwrap_or(ticker.as_str()).to_string();

    // Use sector or industry pairs depending on what this ticker is
    let tickers = if cfg.is_sector(&ticker) {
        cfg.sector_pairs()
    } else {
        cfg.industry_pairs()
    };

    let rrg_all = compute_rrg(&prices, &cfg.benchmark, &tickers, tf);
    let rank_all = compute_rankings(&prices, &cfg.benchmark, &tickers, tf);
    let z_all = compute_zscores(&prices, &cfg.benchmark, &tickers, tf);
    let conv_all = compute_convergence(&rrg_all, &rank_all, &z_all);

    let price_history = build_price_history(&prices, &ticker, &cfg.benchmark);

    Ok(ApiResponse::ok(
        DetailResponse {
            name,
            rrg: rrg_all.into_iter().find(|e| e.ticker == ticker),
            rank: rank_all.into_iter().find(|e| e.ticker == ticker),
            zscore: z_all.into_iter().find(|e| e.ticker == ticker),
            convergence: conv_all.into_iter().find(|e| e.ticker == ticker),
            ticker,
            price_history,
        },
        state.last_updated,
    ))
}

fn build_price_history(prices: &PriceMap, ticker: &str, benchmark: &str) -> Vec<PricePoint> {
    let (sec_series, bch_series) = match (prices.get(ticker), prices.get(benchmark)) {
        (Some(s), Some(b)) => (s, b),
        _ => return vec![],
    };

    let (aligned_sec, aligned_bch) = align(sec_series, bch_series);
    let first_sec = aligned_sec.first().map(|(_, p)| *p).unwrap_or(1.0);
    let first_bch = aligned_bch.first().map(|(_, p)| *p).unwrap_or(1.0);

    aligned_sec
        .iter()
        .zip(aligned_bch.iter())
        .map(|((date, sec_p), (_, bch_p))| {
            let norm_sec = sec_p / first_sec * 100.0;
            let norm_bch = bch_p / first_bch * 100.0;
            PricePoint {
                date: date.to_string(),
                price: (norm_sec * 100.0).round() / 100.0,
                benchmark: (norm_bch * 100.0).round() / 100.0,
                ratio: ((norm_sec / norm_bch) * 1000.0).round() / 1000.0,
            }
        })
        .collect()
}

// ─── GET /api/universe ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct UniverseResponse {
    benchmark: String,
    sectors: Vec<SectorNode>,
}

#[derive(Serialize)]
pub struct SectorNode {
    ticker: String,
    name: String,
    children: Vec<IndustryNode>,
}

#[derive(Serialize)]
pub struct IndustryNode {
    ticker: String,
    name: String,
}

pub async fn get_universe(State(state): State<SharedState>) -> Json<ApiResponse<UniverseResponse>> {
    let state = state.read().await;
    let cfg = &state.config;

    let sectors = cfg
        .sectors
        .iter()
        .map(|(sec_ticker, sec_name)| {
            let children = cfg
                .industry_pairs_for(sec_ticker)
                .into_iter()
                .map(|(t, n)| IndustryNode {
                    ticker: t.to_string(),
                    name: n.to_string(),
                })
                .collect();

            SectorNode {
                ticker: sec_ticker.clone(),
                name: sec_name.clone(),
                children,
            }
        })
        .collect();
    
    let benchmark = cfg.benchmark.clone();
    ApiResponse::ok(UniverseResponse { benchmark, sectors }, None)
}

// ─── GET /api/status ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StatusResponse {
    loaded: bool,
    ticker_count: usize,
    last_updated: Option<String>,
}

pub async fn get_status(State(state): State<SharedState>) -> Json<ApiResponse<StatusResponse>> {
    let state = state.read().await;
    ApiResponse::ok(
        StatusResponse {
            loaded: !state.prices.is_empty(),
            ticker_count: state.prices.len(),
            last_updated: state.last_updated.map(|t| t.to_rfc3339()),
        },
        state.last_updated,
    )
}

// ─── POST /api/refresh ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RefreshResponse {
    tickers_fetched: usize,
    tickers_failed: Vec<String>,
}

pub async fn post_refresh(
    State(state): State<SharedState>,
) -> Result<Json<ApiResponse<RefreshResponse>>, ApiError> {
    let (tickers_owned, db) = {
        let state = state.read().await;
        let tickers_owned: Vec<String> = state
            .config
            .all_tickers()
            .into_iter()
            .map(str::to_string)
            .collect();
        (tickers_owned, state.db.clone())
    };

    let ticker_refs: Vec<&str> = tickers_owned.iter().map(String::as_str).collect();

    // Query the cache for the latest date per ticker
    let cached_dates = db.latest_dates().await.unwrap_or_default();
    tracing::info!(
        "Starting incremental refresh for {} tickers ({} already cached)...",
        ticker_refs.len(),
        cached_dates.len()
    );

    let new_prices = fetch_incremental(&ticker_refs, &cached_dates).await;
    let fetched = new_prices.len();

    let failed: Vec<String> = ticker_refs
        .iter()
        .filter(|t| !new_prices.contains_key(**t) && !cached_dates.contains_key(**t))
        .map(|t| t.to_string())
        .collect();

    let now = chrono::Utc::now();

    // Persist new rows to SQLite (upsert — no deletion)
    if !new_prices.is_empty() {
        if let Err(e) = db.upsert_prices(&new_prices, now).await {
            tracing::error!("Failed to upsert prices to cache: {}", e);
        }
    }

    {
        let mut state = state.write().await;
        merge_prices(&mut state.prices, new_prices);
        state.last_updated = Some(now);
    }

    tracing::info!(
        "Refresh complete: {} tickers updated, {} failed",
        fetched,
        failed.len()
    );

    Ok(ApiResponse::ok(
        RefreshResponse {
            tickers_fetched: fetched,
            tickers_failed: failed,
        },
        Some(now),
    ))
}
