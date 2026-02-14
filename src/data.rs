/// data.rs — Fetch weekly adjusted close prices from Yahoo Finance v8 API,
/// with SQLite caching so data survives restarts.
///
/// On refresh the cache is checked for existing data per ticker.
/// Only the delta (from the latest cached date to today) is fetched,
/// merged with existing rows, and persisted back.
use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rand::Rng;
use reqwest::Client;
use serde::Deserialize;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::collections::{BTreeMap, HashMap};
use tracing::{info, warn};

// ─── Yahoo Finance v8 API response structures ────────────────────────────────

#[derive(Deserialize)]
struct YfResponse {
    chart: YfChart,
}

#[derive(Deserialize)]
struct YfChart {
    result: Option<Vec<YfResult>>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct YfResult {
    timestamp: Vec<i64>,
    indicators: YfIndicators,
}

#[derive(Deserialize)]
struct YfIndicators {
    adjclose: Option<Vec<YfAdjClose>>,
    quote: Vec<YfQuote>,
}

#[derive(Deserialize)]
struct YfAdjClose {
    adjclose: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct YfQuote {
    close: Vec<Option<f64>>,
}

// ─── Public types ────────────────────────────────────────────────────────────

/// (date, adjusted_close) price series for one ticker
pub type PriceSeries = Vec<(NaiveDate, f64)>;

/// Map of ticker → weekly price series (Friday close)
pub type WeeklyPrices = HashMap<String, PriceSeries>;

// ─── SQLite cache ────────────────────────────────────────────────────────────

/// Persistent price cache backed by SQLite.
#[derive(Clone)]
pub struct PriceDb {
    pool: Pool<Sqlite>,
}

impl PriceDb {
    /// Open (or create) the SQLite database at `db_path` and run embedded migrations.
    pub async fn open(db_path: &str) -> Result<Self> {
        let url = format!("sqlite:{}?mode=rwc", db_path);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await?;

        sqlx::migrate!().run(&pool).await?;

        Ok(Self { pool })
    }

    /// Return the latest cached date for each ticker that has data.
    pub async fn latest_dates(&self) -> Result<HashMap<String, NaiveDate>> {
        let rows = sqlx::query!(
            "SELECT ticker, MAX(date) as max_date FROM prices GROUP BY ticker"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut map = HashMap::new();
        for row in rows {
            if let Some(ticker) = row.ticker {
                if let Ok(date) = row.max_date.parse::<NaiveDate>() {
                    map.insert(ticker, date);
                }
            }
        }
        Ok(map)
    }

    /// Upsert price rows — inserts new rows and updates existing ones on conflict.
    pub async fn upsert_prices(
        &self,
        prices: &WeeklyPrices,
        last_updated: DateTime<Utc>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for (ticker, series) in prices {
            for (date, close) in series {
                let date_str = date.to_string();
                sqlx::query!(
                    "INSERT INTO prices (ticker, date, close) VALUES (?, ?, ?)
                     ON CONFLICT(ticker, date) DO UPDATE SET close = excluded.close",
                    ticker,
                    date_str,
                    close,
                )
                .execute(&mut *tx)
                .await?;
            }
        }

        let ts = last_updated.to_rfc3339();
        sqlx::query!(
            "INSERT INTO metadata (key, value) VALUES ('last_updated', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            ts,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        info!("Upserted {} tickers into cache", prices.len());
        Ok(())
    }

    /// Load all cached prices. Returns `None` if the cache is empty.
    pub async fn load_prices(&self) -> Result<Option<(WeeklyPrices, DateTime<Utc>)>> {
        let row = sqlx::query_scalar!(
            "SELECT value FROM metadata WHERE key = 'last_updated'"
        )
        .fetch_optional(&self.pool)
        .await?;

        let last_updated = match row {
            Some(s) => s.parse::<DateTime<Utc>>()?,
            None => return Ok(None),
        };

        let rows = sqlx::query!(
            "SELECT ticker, date, close FROM prices ORDER BY ticker, date"
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut prices = WeeklyPrices::new();
        for row in rows {
            let date = row.date.parse::<NaiveDate>()?;
            let close = row.close;
            prices.entry(row.ticker).or_default().push((date, close));
        }

        Ok(Some((prices, last_updated)))
    }
}

// ─── Fetching ────────────────────────────────────────────────────────────────

/// Incrementally fetch weekly prices for all tickers.
///
/// `cached_dates` maps each ticker to the latest date already in the cache.
/// For tickers present in `cached_dates`, only data *after* that date is fetched.
/// For tickers not in the map, the full 2-year history is fetched.
///
/// Returns **only the newly fetched rows** (the caller merges them into the
/// existing in-memory map and persists them via `upsert_prices`).
pub async fn fetch_incremental(
    tickers: &[&str],
    cached_dates: &HashMap<String, NaiveDate>,
) -> WeeklyPrices {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let today = Utc::now().date_naive();
    let mut results = WeeklyPrices::new();

    for ticker in tickers {
        if let Some(&latest) = cached_dates.get(*ticker) {
            if latest >= today {
                info!("{} already up-to-date (cached through {}), skipping", ticker, latest);
                continue;
            }
        }

        let since = cached_dates.get(*ticker).copied();
        match fetch_weekly(&client, ticker, since).await {
            Ok(series) => {
                info!(
                    "Fetched {} new data points for {} (since {:?})",
                    series.len(),
                    ticker,
                    since
                );
                if !series.is_empty() {
                    results.insert(ticker.to_string(), series);
                }
            }
            Err(e) => {
                warn!("Failed to fetch {}: {}", ticker, e);
            }
        }
        let delay = rand::rng().random_range(100..=300);
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }

    results
}

/// Fetch daily prices for a single ticker and resample to weekly (Friday close).
///
/// If `since` is `Some(date)`, only data from that date onward is requested
/// (using Yahoo's period1/period2 timestamp params).  Otherwise the full
/// 2-year range is fetched.
///
/// Retries up to `MAX_RETRIES` times with exponential backoff on transient errors.
const MAX_RETRIES: u32 = 2;

async fn fetch_weekly(
    client: &Client,
    ticker: &str,
    since: Option<NaiveDate>,
) -> Result<PriceSeries> {
    let url = match since {
        Some(date) => {
            let period1 = date
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp();
            let period2 = Utc::now().timestamp();
            format!(
                "https://query1.finance.yahoo.com/v8/finance/chart/{}?interval=1d&period1={}&period2={}",
                ticker, period1, period2
            )
        }
        None => format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{}?interval=1d&range=2y",
            ticker
        ),
    };

    let mut last_err = anyhow!("fetch failed for {}", ticker);

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let backoff = 2u64.pow(attempt) * 500;
            warn!("{}: retry {}/{} in {}ms", ticker, attempt, MAX_RETRIES, backoff);
            tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
        }

        let response = match client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = e.into();
                continue;
            }
        };

        // Retry on 429 (rate-limited) or 5xx server errors
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            last_err = anyhow!("{}: HTTP {}", ticker, status);
            continue;
        }

        let resp: YfResponse = match response.json().await {
            Ok(r) => r,
            Err(e) => {
                last_err = e.into();
                continue;
            }
        };

        if let Some(err) = resp.chart.error {
            return Err(anyhow!("Yahoo Finance API error for {}: {}", ticker, err));
        }

        let result = resp
            .chart
            .result
            .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .ok_or_else(|| anyhow!("No data returned for {}", ticker))?;

        // Prefer adjclose, fall back to regular close
        let closes = result
            .indicators
            .adjclose
            .as_deref()
            .and_then(|a| a.first())
            .map(|a| &a.adjclose)
            .or_else(|| result.indicators.quote.first().map(|q| &q.close))
            .cloned()
            .unwrap_or_default();

        // Build daily (date, price) series — skip entries with missing prices
        let daily: Vec<(NaiveDate, f64)> = result
            .timestamp
            .iter()
            .zip(closes)
            .filter_map(|(&ts, price)| {
                let date = DateTime::from_timestamp(ts, 0)?.naive_local().date();
                Some((date, price?))
            })
            .collect();

        // Only keep days strictly after `since` — the cached date itself already exists
        let filtered: Vec<(NaiveDate, f64)> = match since {
            Some(cutoff) => daily.into_iter().filter(|(d, _)| *d > cutoff).collect(),
            None => daily,
        };

        return Ok(resample_weekly(&filtered));
    }

    Err(last_err)
}

/// From a sorted daily (date, price) series, produce a weekly series.
/// Keeps the last available trading day in each ISO week (typically Friday).
fn resample_weekly(daily: &[(NaiveDate, f64)]) -> PriceSeries {
    let mut by_week: BTreeMap<(i32, u32), (NaiveDate, f64)> = BTreeMap::new();

    for &(date, price) in daily {
        let key = (date.iso_week().year(), date.iso_week().week());
        by_week
            .entry(key)
            .and_modify(|(d, p)| {
                if date > *d {
                    *d = date;
                    *p = price;
                }
            })
            .or_insert((date, price));
    }

    by_week.into_values().collect()
}

// ─── Merge helper ────────────────────────────────────────────────────────────

/// Merge newly fetched rows into the existing in-memory price map.
/// New rows for each ticker are appended (or overwritten for same-date duplicates),
/// then sorted by date.
pub fn merge_prices(existing: &mut WeeklyPrices, new: WeeklyPrices) {
    for (ticker, new_series) in new {
        let entry = existing.entry(ticker).or_default();
        let existing_map: HashMap<NaiveDate, f64> = entry.iter().copied().collect();
        let mut merged: BTreeMap<NaiveDate, f64> = existing_map.into_iter().collect();
        for (date, price) in new_series {
            merged.insert(date, price);
        }
        *entry = merged.into_iter().collect();
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Extract just the price values from a PriceSeries.
pub fn prices_only(series: &PriceSeries) -> Vec<f64> {
    series.iter().map(|(_, p)| *p).collect()
}

/// Align two price series to the same date range (inner join on dates).
pub fn align(a: &PriceSeries, b: &PriceSeries) -> (PriceSeries, PriceSeries) {
    let b_map: HashMap<NaiveDate, f64> = b.iter().copied().collect();

    let (aligned_a, aligned_b): (Vec<_>, Vec<_>) = a
        .iter()
        .filter_map(|&(date, pa)| {
            let pb = b_map.get(&date)?;
            Some(((date, pa), (date, *pb)))
        })
        .unzip();

    (aligned_a, aligned_b)
}
