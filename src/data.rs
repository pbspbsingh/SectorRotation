/// data.rs — Fetch weekly adjusted close prices from Yahoo Finance v8 API
use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
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

// ─── Fetching ────────────────────────────────────────────────────────────────

/// Fetch ~2 years of weekly adjusted prices for all tickers.
/// Uses Yahoo Finance v8 chart endpoint (no API key required).
pub async fn fetch_all_weekly(tickers: &[&str]) -> WeeklyPrices {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let mut results = WeeklyPrices::new();

    for ticker in tickers {
        match fetch_weekly(&client, ticker).await {
            Ok(series) => {
                info!("Fetched {} data points for {}", series.len(), ticker);
                results.insert(ticker.to_string(), series);
            }
            Err(e) => {
                warn!("Failed to fetch {}: {}", ticker, e);
            }
        }
        // Small delay to avoid rate limiting
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    results
}

/// Fetch daily prices for a single ticker and resample to weekly (Friday close).
async fn fetch_weekly(client: &Client, ticker: &str) -> Result<PriceSeries> {
    // Use daily interval and 2y range — we resample to weekly ourselves
    // for better control over which day we use as the weekly close
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}?interval=1d&range=2y",
        ticker
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?
        .json::<YfResponse>()
        .await?;

    if let Some(err) = resp.chart.error {
        return Err(anyhow!("Yahoo Finance API error: {}", err));
    }

    let result = resp
        .chart
        .result
        .ok_or_else(|| anyhow!("No data returned"))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Empty result array"))?;

    // Parse timestamps + adjusted closes
    let closes: Vec<Option<f64>> = result
        .indicators
        .adjclose
        .as_ref()
        .and_then(|a| a.first())
        .map(|a| a.adjclose.clone())
        .unwrap_or_else(|| {
            // Fall back to regular close if adjclose not present
            result
                .indicators
                .quote
                .first()
                .map(|q| q.close.clone())
                .unwrap_or_default()
        });

    // Build (date, price) daily series
    let daily: Vec<(NaiveDate, f64)> = result
        .timestamp
        .iter()
        .zip(closes.iter())
        .filter_map(|(ts, price)| {
            let date = DateTime::from_timestamp(*ts, 0)?;
            let date = date.naive_local().date();
            price.map(|p| (date, p))
        })
        .collect();

    // Resample: keep only Friday closes (or last trading day of each week)
    let weekly = resample_weekly(&daily);

    Ok(weekly)
}

/// From a sorted daily (date, price) series, produce a weekly series.
/// Uses the last available trading day in each ISO week (typically Friday).
fn resample_weekly(daily: &[(NaiveDate, f64)]) -> PriceSeries {
    let mut weekly: HashMap<(i32, u32), (NaiveDate, f64)> = HashMap::new();

    for (date, price) in daily {
        let key = (date.iso_week().year(), date.iso_week().week());
        // Keep the latest date in the week (overwrite earlier days)
        weekly
            .entry(key)
            .and_modify(|(d, p)| {
                if date > d {
                    *d = *date;
                    *p = *price;
                }
            })
            .or_insert((*date, *price));
    }

    let mut result: PriceSeries = weekly.into_values().collect();
    result.sort_by_key(|(d, _)| *d);
    result
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Extract just the price values from a PriceSeries
pub fn prices_only(series: &PriceSeries) -> Vec<f64> {
    series.iter().map(|(_, p)| *p).collect()
}

/// Extract just the dates from a PriceSeries
pub fn dates_only(series: &PriceSeries) -> Vec<NaiveDate> {
    series.iter().map(|(d, _)| *d).collect()
}

/// Align two price series to the same date range (inner join on dates)
pub fn align(a: &PriceSeries, b: &PriceSeries) -> (PriceSeries, PriceSeries) {
    let b_map: HashMap<NaiveDate, f64> = b.iter().cloned().collect();

    let mut aligned_a = Vec::new();
    let mut aligned_b = Vec::new();

    for (date, price_a) in a {
        if let Some(price_b) = b_map.get(date) {
            aligned_a.push((*date, *price_a));
            aligned_b.push((*date, *price_b));
        }
    }

    (aligned_a, aligned_b)
}
