/// config.rs — Load ticker universe from config.toml at startup.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

// ─── TOML deserialization types ───────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct TickerEntry {
    pub ticker: String,
    pub name: String,
    /// TradingView FactSet sectors (populated for [[sectors]] entries)
    #[serde(default)]
    pub tv_sectors: Vec<String>,
    /// TradingView FactSet industries (populated for [[industry_groups.*]] entries)
    #[serde(default)]
    pub tv_industries: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    benchmark: String,
    sectors: Vec<TickerEntry>,
    #[serde(default)]
    industry_groups: HashMap<String, Vec<TickerEntry>>,
}

// ─── Public Config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub benchmark: String,
    /// Ordered list of sector entries (preserves config.toml order).
    pub sectors: Vec<TickerEntry>,
    /// Map of sector_ticker → Vec<industry TickerEntry>.
    pub industry_groups: HashMap<String, Vec<TickerEntry>>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read config file: {path}"))?;

        let file: ConfigFile =
            toml::from_str(&raw).with_context(|| format!("Failed to parse config file: {path}"))?;

        // Validate: every industry_groups key must reference a known sector ticker
        let sector_tickers: std::collections::HashSet<&str> =
            file.sectors.iter().map(|s| s.ticker.as_str()).collect();

        for key in file.industry_groups.keys() {
            if !sector_tickers.contains(key.as_str()) {
                anyhow::bail!(
                    "config.toml: [industry_groups.{key}] references unknown sector '{key}'. \
                     Add a matching [[sectors]] entry first."
                );
            }
        }

        Ok(Self {
            benchmark: file.benchmark,
            sectors: file.sectors,
            industry_groups: file.industry_groups,
        })
    }

    // ─── Ticker accessors ─────────────────────────────────────────────────────

    pub fn sector_tickers(&self) -> Vec<&str> {
        self.sectors.iter().map(|e| e.ticker.as_str()).collect()
    }

    pub fn industry_tickers(&self) -> Vec<&str> {
        self.industry_groups
            .values()
            .flatten()
            .map(|e| e.ticker.as_str())
            .collect()
    }

    pub fn all_tickers(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        let mut v = Vec::new();
        for t in std::iter::once(self.benchmark.as_str())
            .chain(self.sector_tickers())
            .chain(self.industry_tickers())
        {
            if seen.insert(t) {
                v.push(t);
            }
        }
        v
    }

    // ─── Pair accessors (for compute functions) ───────────────────────────────

    /// (ticker, name) pairs for all sectors — used by compute functions.
    pub fn sector_pairs(&self) -> Vec<(&str, &str)> {
        self.sectors
            .iter()
            .map(|e| (e.ticker.as_str(), e.name.as_str()))
            .collect()
    }

    /// (ticker, name) pairs for all industry groups (flattened).
    pub fn industry_pairs(&self) -> Vec<(&str, &str)> {
        self.industry_groups
            .values()
            .flatten()
            .map(|e| (e.ticker.as_str(), e.name.as_str()))
            .collect()
    }

    // ─── Lookup helpers ───────────────────────────────────────────────────────

    pub fn name_of(&self, ticker: &str) -> Option<&str> {
        self.sectors
            .iter()
            .find(|e| e.ticker == ticker)
            .map(|e| e.name.as_str())
            .or_else(|| {
                self.industry_groups
                    .values()
                    .flatten()
                    .find(|e| e.ticker == ticker)
                    .map(|e| e.name.as_str())
            })
    }

    pub fn is_sector(&self, ticker: &str) -> bool {
        self.sectors.iter().any(|e| e.ticker == ticker)
    }
}
