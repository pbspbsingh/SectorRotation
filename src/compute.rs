/// compute.rs — RRG, RS Rankings, and Z-Score computation
use crate::data::{align, prices_only, WeeklyPrices};
use serde::Serialize;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// SHARED OUTPUT TYPES
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RrgPoint {
    pub rs_ratio: f64,
    pub rs_momentum: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RrgEntry {
    pub ticker: String,
    pub name: String,
    pub current: RrgPoint,
    /// Last N weeks of (rs_ratio, rs_momentum) — oldest first
    pub tail: Vec<RrgPoint>,
    pub quadrant: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankEntry {
    pub ticker: String,
    pub name: String,
    pub rel_ret_20d: f64,
    pub rel_ret_63d: f64,
    pub rel_ret_126d: f64,
    pub rank_20d: usize,
    pub rank_63d: usize,
    pub rank_126d: usize,
    /// Rank 4 weeks ago for 20d window (positive = improved)
    pub rank_change: i32,
    /// Signal: "rising", "falling", "stable"
    pub trend: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZScoreEntry {
    pub ticker: String,
    pub name: String,
    pub z_short: f64,
    pub z_long: f64,
    /// Signal: "leader", "improving", "lagging", "reverting"
    pub signal: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvergenceEntry {
    pub ticker: String,
    pub name: String,
    pub rrg_signal: bool,
    pub rank_signal: bool,
    pub zscore_signal: bool,
    /// "high", "medium", "low"
    pub confidence: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// MATH UTILITIES
// ─────────────────────────────────────────────────────────────────────────────

/// Exponential Moving Average (EMA)
/// span: number of periods, equivalent to 2/(span+1) smoothing factor
fn ema(values: &[f64], span: usize) -> Vec<f64> {
    if values.is_empty() {
        return vec![];
    }
    let alpha = 2.0 / (span as f64 + 1.0);
    let mut result = vec![0.0; values.len()];
    result[0] = values[0];
    for i in 1..values.len() {
        result[i] = alpha * values[i] + (1.0 - alpha) * result[i - 1];
    }
    result
}


/// Cross-sectional Z-score: (x - mean) / std across a slice of values
fn zscore_vec(values: &[f64]) -> Vec<f64> {
    if values.len() < 2 {
        return vec![0.0; values.len()];
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    let std = variance.sqrt();
    if std < 1e-10 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|v| (v - mean) / std).collect()
}

/// Rank a slice of (index, value) pairs — rank 1 = highest value
fn rank_descending(values: &[f64]) -> Vec<usize> {
    let mut indexed: Vec<(usize, f64)> = values.iter().cloned().enumerate().collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut ranks = vec![0usize; values.len()];
    for (rank, (idx, _)) in indexed.iter().enumerate() {
        ranks[*idx] = rank + 1;
    }
    ranks
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ─────────────────────────────────────────────────────────────────────────────
// METHOD 1: RELATIVE ROTATION GRAPH (RRG)
// ─────────────────────────────────────────────────────────────────────────────

/// EMA span parameters for JdK RS-Ratio (approximation)
const EMA_SHORT_SPAN: usize = 10; // ~10 weeks short
const EMA_LONG_SPAN: usize = 40; // ~40 weeks long
const MOM_LOOKBACK: usize = 4; // de Kempenaer standard: 4-week RoC
const TAIL_WEEKS: usize = 5; // standard 5-week tail

pub fn compute_rrg(
    prices: &WeeklyPrices,
    benchmark: &str,
    tickers: &[(&str, &str)], // (ticker, name)
) -> Vec<RrgEntry> {
    let bench_series = match prices.get(benchmark) {
        Some(s) => s,
        None => return vec![],
    };

    let mut entries = Vec::new();

    for (ticker, name) in tickers {
        let sector_series = match prices.get(*ticker) {
            Some(s) => s,
            None => continue,
        };

        // Align on same dates
        let (aligned_sector, aligned_bench) = align(sector_series, bench_series);
        if aligned_sector.len() < EMA_LONG_SPAN + MOM_LOOKBACK + TAIL_WEEKS + 5 {
            continue; // Not enough data
        }

        let sec_prices = prices_only(&aligned_sector);
        let bch_prices = prices_only(&aligned_bench);

        // Step 1: Relative Strength ratio
        let rs: Vec<f64> = sec_prices
            .iter()
            .zip(bch_prices.iter())
            .map(|(s, b)| s / b)
            .collect();

        // Step 2: Two EMAs of RS
        let ema_short = ema(&rs, EMA_SHORT_SPAN);
        let ema_long = ema(&rs, EMA_LONG_SPAN);

        // Step 3: RS-Ratio = 100 + (EMA_short - EMA_long) / EMA_long * 100
        let rs_ratio: Vec<f64> = ema_short
            .iter()
            .zip(ema_long.iter())
            .map(|(s, l)| 100.0 + (s - l) / l * 100.0)
            .collect();

        // Step 4: RS-Momentum = 100 + RoC(RS-Ratio, 4 weeks)
        // Only compute where we have MOM_LOOKBACK lookback
        let rs_momentum: Vec<f64> = (MOM_LOOKBACK..rs_ratio.len())
            .map(|i| {
                let past = rs_ratio[i - MOM_LOOKBACK];
                if past.abs() < 1e-10 {
                    100.0
                } else {
                    100.0 + (rs_ratio[i] - past) / past * 100.0
                }
            })
            .collect();

        // Align rs_ratio to same length as rs_momentum
        let ratio_aligned = &rs_ratio[MOM_LOOKBACK..];

        // Step 5: Extract tail (last TAIL_WEEKS points)
        let tail_len = TAIL_WEEKS.min(rs_momentum.len());
        let tail: Vec<RrgPoint> = ratio_aligned[ratio_aligned.len() - tail_len..]
            .iter()
            .zip(rs_momentum[rs_momentum.len() - tail_len..].iter())
            .map(|(r, m)| RrgPoint {
                rs_ratio: round2(*r),
                rs_momentum: round2(*m),
            })
            .collect();

        let current = match tail.last() {
            Some(p) => p.clone(),
            None => continue,
        };

        let quadrant = classify_quadrant(current.rs_ratio, current.rs_momentum);

        entries.push(RrgEntry {
            ticker: ticker.to_string(),
            name: name.to_string(),
            current,
            tail,
            quadrant,
        });
    }

    entries
}

fn classify_quadrant(ratio: f64, momentum: f64) -> String {
    match (ratio >= 100.0, momentum >= 100.0) {
        (true, true) => "Leading".to_string(),
        (true, false) => "Weakening".to_string(),
        (false, false) => "Lagging".to_string(),
        (false, true) => "Improving".to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// METHOD 2: ROLLING RELATIVE STRENGTH RANKINGS
// ─────────────────────────────────────────────────────────────────────────────

const WINDOW_SHORT: usize = 20; // ~1 month (trading days → weekly ≈ 4 weeks)
const WINDOW_MED: usize = 63; // ~3 months (weekly ≈ 13 weeks)
const WINDOW_LONG: usize = 126; // ~6 months (weekly ≈ 26 weeks)
const RANK_CHANGE_LOOKBACK: usize = 4; // Compare rank vs 4 weeks ago

pub fn compute_rankings(
    prices: &WeeklyPrices,
    benchmark: &str,
    tickers: &[(&str, &str)],
) -> Vec<RankEntry> {
    let bench_series = match prices.get(benchmark) {
        Some(s) => s,
        None => return vec![],
    };

    let _bench_prices = prices_only(bench_series);

    // Compute relative returns for each ticker across 3 windows
    struct TempEntry {
        ticker: String,
        name: String,
        rel_20: f64,
        rel_63: f64,
        rel_126: f64,
        // For rank change: relative return 4 weeks ago
        rel_20_past: f64,
    }

    let mut temps: Vec<TempEntry> = Vec::new();

    for (ticker, name) in tickers {
        let series = match prices.get(*ticker) {
            Some(s) => s,
            None => continue,
        };

        let (aligned_sec, aligned_bench) = align(series, bench_series);
        if aligned_sec.len() < WINDOW_LONG + RANK_CHANGE_LOOKBACK + 5 {
            continue;
        }

        let sec = prices_only(&aligned_sec);
        let bch = prices_only(&aligned_bench);
        let n = sec.len();

        let rel_ret = |window: usize, offset: usize| -> f64 {
            if n < window + offset + 1 {
                return 0.0;
            }
            let now = n - 1 - offset;
            let past = now.saturating_sub(window);
            let sr = (sec[now] - sec[past]) / sec[past] * 100.0;
            let br = (bch[now] - bch[past]) / bch[past] * 100.0;
            round2(sr - br)
        };

        temps.push(TempEntry {
            ticker: ticker.to_string(),
            name: name.to_string(),
            rel_20: rel_ret(WINDOW_SHORT, 0),
            rel_63: rel_ret(WINDOW_MED, 0),
            rel_126: rel_ret(WINDOW_LONG, 0),
            rel_20_past: rel_ret(WINDOW_SHORT, RANK_CHANGE_LOOKBACK),
        });
    }

    if temps.is_empty() {
        return vec![];
    }

    // Rank current relative returns (1 = best)
    let rels_20: Vec<f64> = temps.iter().map(|t| t.rel_20).collect();
    let rels_63: Vec<f64> = temps.iter().map(|t| t.rel_63).collect();
    let rels_126: Vec<f64> = temps.iter().map(|t| t.rel_126).collect();
    let rels_20p: Vec<f64> = temps.iter().map(|t| t.rel_20_past).collect();

    let ranks_20 = rank_descending(&rels_20);
    let ranks_63 = rank_descending(&rels_63);
    let ranks_126 = rank_descending(&rels_126);
    let ranks_20_past = rank_descending(&rels_20p);

    temps
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let rank_now = ranks_20[i] as i32;
            let rank_past = ranks_20_past[i] as i32;
            let rank_change = rank_past - rank_now; // positive = improved (rank number went down)

            let trend = if rank_change >= 3 {
                "rising".to_string()
            } else if rank_change <= -3 {
                "falling".to_string()
            } else {
                "stable".to_string()
            };

            RankEntry {
                ticker: t.ticker,
                name: t.name,
                rel_ret_20d: t.rel_20,
                rel_ret_63d: t.rel_63,
                rel_ret_126d: t.rel_126,
                rank_20d: ranks_20[i],
                rank_63d: ranks_63[i],
                rank_126d: ranks_126[i],
                rank_change,
                trend,
            }
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// METHOD 3: CROSS-SECTIONAL MOMENTUM Z-SCORE
// ─────────────────────────────────────────────────────────────────────────────

const ZSCORE_SHORT: usize = 20; // ~1 month
const ZSCORE_LONG: usize = 63; // ~3 months

pub fn compute_zscores(
    prices: &WeeklyPrices,
    benchmark: &str,
    tickers: &[(&str, &str)],
) -> Vec<ZScoreEntry> {
    let bench_series = match prices.get(benchmark) {
        Some(s) => s,
        None => return vec![],
    };

    struct Temp {
        ticker: String,
        name: String,
        rel_short: f64,
        rel_long: f64,
    }

    let mut temps: Vec<Temp> = Vec::new();

    for (ticker, name) in tickers {
        let series = match prices.get(*ticker) {
            Some(s) => s,
            None => continue,
        };

        let (aligned_sec, aligned_bench) = align(series, bench_series);
        if aligned_sec.len() < ZSCORE_LONG + 5 {
            continue;
        }

        let sec = prices_only(&aligned_sec);
        let bch = prices_only(&aligned_bench);
        let n = sec.len();

        let rel_ret = |window: usize| -> f64 {
            let past = n - 1 - window;
            let sr = (sec[n - 1] - sec[past]) / sec[past] * 100.0;
            let br = (bch[n - 1] - bch[past]) / bch[past] * 100.0;
            sr - br
        };

        temps.push(Temp {
            ticker: ticker.to_string(),
            name: name.to_string(),
            rel_short: rel_ret(ZSCORE_SHORT),
            rel_long: rel_ret(ZSCORE_LONG),
        });
    }

    if temps.is_empty() {
        return vec![];
    }

    // Cross-sectional z-scores
    let rels_short: Vec<f64> = temps.iter().map(|t| t.rel_short).collect();
    let rels_long: Vec<f64> = temps.iter().map(|t| t.rel_long).collect();

    let z_shorts = zscore_vec(&rels_short);
    let z_longs = zscore_vec(&rels_long);

    temps
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let zs = round2(z_shorts[i]);
            let zl = round2(z_longs[i]);

            // Signal classification
            let signal = if zs > 1.5 && zl > 0.5 {
                "leader" // Strong across both windows
            } else if zs > 0.5 && zl < 0.0 {
                "improving" // Short-term improving, long-term still weak — early rotation
            } else if zs < -1.5 && zl < -0.5 {
                "lagging" // Weak across both
            } else if zs > 0.0 && zl < -1.5 {
                "reverting" // Recovering from deep lows — mean reversion / early entry
            } else {
                "neutral"
            };

            ZScoreEntry {
                ticker: t.ticker,
                name: t.name,
                z_short: zs,
                z_long: zl,
                signal: signal.to_string(),
            }
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// CONVERGENCE SIGNAL — combines all 3 methods
// ─────────────────────────────────────────────────────────────────────────────

pub fn compute_convergence(
    rrg: &[RrgEntry],
    rankings: &[RankEntry],
    zscores: &[ZScoreEntry],
) -> Vec<ConvergenceEntry> {
    // Index by ticker for quick lookup
    let rank_map: HashMap<&str, &RankEntry> =
        rankings.iter().map(|r| (r.ticker.as_str(), r)).collect();
    let z_map: HashMap<&str, &ZScoreEntry> =
        zscores.iter().map(|z| (z.ticker.as_str(), z)).collect();

    rrg.iter()
        .filter_map(|r| {
            let rank = rank_map.get(r.ticker.as_str())?;
            let z = z_map.get(r.ticker.as_str())?;

            // RRG signal: in Improving or Leading quadrant, and tail moving right (ratio increasing)
            let rrg_signal = matches!(r.quadrant.as_str(), "Improving" | "Leading")
                && r.tail.len() >= 2
                && r.tail.last().unwrap().rs_ratio
                    > r.tail[r.tail.len().saturating_sub(2)].rs_ratio;

            // Rank signal: short-term rank improving
            let rank_signal = rank.rank_change >= 2;

            // Z-score signal: either leader or recovering from lows
            let zscore_signal = matches!(z.signal.as_str(), "leader" | "improving" | "reverting");

            let score = [rrg_signal, rank_signal, zscore_signal]
                .iter()
                .filter(|&&v| v)
                .count();

            let confidence = match score {
                3 => "high",
                2 => "medium",
                _ => "low",
            };

            Some(ConvergenceEntry {
                ticker: r.ticker.clone(),
                name: r.name.clone(),
                rrg_signal,
                rank_signal,
                zscore_signal,
                confidence: confidence.to_string(),
            })
        })
        .collect()
}
