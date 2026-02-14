/// compute.rs — RRG, RS Rankings, and Z-Score computation
use crate::data::{align, prices_only, PriceMap};
use serde::Serialize;
use std::collections::HashMap;
use tracing::warn;

// ─────────────────────────────────────────────────────────────────────────────
// TIMEFRAME PARAMETERS
// ─────────────────────────────────────────────────────────────────────────────

/// Window parameters that depend on the bar frequency (daily vs weekly).
#[derive(Debug, Clone, Copy)]
pub struct Timeframe {
    pub ema_short: usize,
    pub ema_long: usize,
    pub mom_lookback: usize,
    pub tail_len: usize,
    pub win_short: usize,
    pub win_med: usize,
    pub win_long: usize,
    pub rank_change_lookback: usize,
    pub z_short: usize,
    pub z_long: usize,
}

/// Parameters calibrated for DAILY bars.
/// Each value is the weekly equivalent × 5 trading days.
pub const DAILY: Timeframe = Timeframe {
    // RRG — EMA spans (10w and 40w converted to days)
    ema_short: 50, // 10 weeks × 5
    ema_long: 200, // 40 weeks × 5

    // RRG — momentum lookback (de Kempenaer: 4 weeks)
    mom_lookback: 20, // 4 weeks × 5

    // RRG — tail history shown on the graph (de Kempenaer: 5 weeks)
    tail_len: 25, // 5 weeks × 5

    // Rankings — relative return windows
    win_short: 20, // 1 month  (~4 weeks)
    win_med: 63,   // 1 quarter (~13 weeks)
    win_long: 126, // 6 months  (~26 weeks)

    // Rankings — lookback for rank change detection (4 weeks ago)
    rank_change_lookback: 20, // 4 weeks × 5

    // Z-Score — relative return windows
    z_short: 20, // 1 month
    z_long: 63,  // 1 quarter
};

/// Parameters calibrated for WEEKLY bars.
/// These are de Kempenaer's original published defaults.
pub const WEEKLY: Timeframe = Timeframe {
    // RRG
    ema_short: 10,   // 10 weeks (~2.5 months)
    ema_long: 40,    // 40 weeks (~10 months)
    mom_lookback: 4, // 4 weeks  (de Kempenaer standard)
    tail_len: 5,     // 5 weeks  (de Kempenaer standard)

    // Rankings
    win_short: 4, // 1 month
    win_med: 13,  // 1 quarter
    win_long: 26, // 6 months

    // Rankings
    rank_change_lookback: 4, // 4 weeks ago

    // Z-Score
    z_short: 4, // 1 month
    z_long: 13, // 1 quarter
};

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
    /// Last N periods of (rs_ratio, rs_momentum) — oldest first
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
    /// Rank change over lookback window (positive = improved)
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

fn ema(values: &[f64], span: usize) -> Vec<f64> {
    let n = values.len();
    if span == 0 || n < span {
        return vec![f64::NAN; n];
    }

    let alpha = 2.0 / (span as f64 + 1.0);
    let initial_sma = values[..span].iter().sum::<f64>() / (span as f64);
    let mut result = vec![initial_sma; n];
    let mut prev = initial_sma;
    for i in span..n {
        let cur = (values[i] - prev).mul_add(alpha, prev);
        result[i] = cur;
        prev = cur;
    }
    result
}

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

pub fn compute_rrg(
    prices: &PriceMap,
    benchmark: &str,
    tickers: &[(&str, &str)],
    tf: &Timeframe,
) -> Vec<RrgEntry> {
    let bench_series = match prices.get(benchmark) {
        Some(s) => s,
        None => return vec![],
    };

    let mut entries = Vec::new();

    for &(ticker, name) in tickers {
        let sector_series = match prices.get(ticker) {
            Some(s) => s,
            None => continue,
        };

        let (aligned_sector, aligned_bench) = align(sector_series, bench_series);
        if aligned_sector.len() < tf.ema_long + tf.mom_lookback + tf.tail_len + 5 {
            warn!("Less data found for {ticker}, skipping it");
            continue;
        }

        let sec_prices = prices_only(&aligned_sector);
        let bch_prices = prices_only(&aligned_bench);

        let rs: Vec<f64> = sec_prices
            .iter()
            .zip(bch_prices.iter())
            .map(|(s, b)| s / b)
            .collect();

        let ema_short = ema(&rs, tf.ema_short);
        let ema_long = ema(&rs, tf.ema_long);

        let rs_ratio: Vec<f64> = ema_short
            .iter()
            .zip(ema_long.iter())
            .map(|(s, l)| 100.0 + (s - l) / l * 100.0)
            .collect();

        let rs_momentum: Vec<f64> = (tf.mom_lookback..rs_ratio.len())
            .map(|i| {
                let past = rs_ratio[i - tf.mom_lookback];
                if past.abs() < 1e-10 {
                    100.0
                } else {
                    100.0 + (rs_ratio[i] - past) / past * 100.0
                }
            })
            .collect();

        let ratio_aligned = &rs_ratio[tf.mom_lookback..];

        let tail_len = tf.tail_len.min(rs_momentum.len());
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

pub fn compute_rankings(
    prices: &PriceMap,
    benchmark: &str,
    tickers: &[(&str, &str)],
    tf: &Timeframe,
) -> Vec<RankEntry> {
    let bench_series = match prices.get(benchmark) {
        Some(s) => s,
        None => return vec![],
    };

    let _bench_prices = prices_only(bench_series);

    struct TempEntry {
        ticker: String,
        name: String,
        rel_20: f64,
        rel_63: f64,
        rel_126: f64,
        rel_20_past: f64,
    }

    let mut temps: Vec<TempEntry> = Vec::new();

    for (ticker, name) in tickers {
        let series = match prices.get(*ticker) {
            Some(s) => s,
            None => continue,
        };

        let (aligned_sec, aligned_bench) = align(series, bench_series);
        if aligned_sec.len() < tf.win_long + tf.rank_change_lookback + 5 {
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
            rel_20: rel_ret(tf.win_short, 0),
            rel_63: rel_ret(tf.win_med, 0),
            rel_126: rel_ret(tf.win_long, 0),
            rel_20_past: rel_ret(tf.win_short, tf.rank_change_lookback),
        });
    }

    if temps.is_empty() {
        return vec![];
    }

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
            let rank_change = rank_past - rank_now;

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

pub fn compute_zscores(
    prices: &PriceMap,
    benchmark: &str,
    tickers: &[(&str, &str)],
    tf: &Timeframe,
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
        if aligned_sec.len() < tf.z_long + 5 {
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
            rel_short: rel_ret(tf.z_short),
            rel_long: rel_ret(tf.z_long),
        });
    }

    if temps.is_empty() {
        return vec![];
    }

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

            let signal = if zs > 1.5 && zl > 0.5 {
                "leader"
            } else if zs > 0.5 && zl < 0.0 {
                "improving"
            } else if zs < -1.5 && zl < -0.5 {
                "lagging"
            } else if zs > 0.0 && zl < -1.5 {
                "reverting"
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
    let rank_map: HashMap<&str, &RankEntry> =
        rankings.iter().map(|r| (r.ticker.as_str(), r)).collect();
    let z_map: HashMap<&str, &ZScoreEntry> =
        zscores.iter().map(|z| (z.ticker.as_str(), z)).collect();

    rrg.iter()
        .filter_map(|r| {
            let rank = rank_map.get(r.ticker.as_str())?;
            let z = z_map.get(r.ticker.as_str())?;

            let rrg_signal = matches!(r.quadrant.as_str(), "Improving" | "Leading")
                && r.tail.len() >= 2
                && r.tail.last().unwrap().rs_ratio
                    > r.tail[r.tail.len().saturating_sub(2)].rs_ratio;

            let rank_signal = rank.rank_change >= 2;

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
