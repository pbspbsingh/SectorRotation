/// config.rs — Load ticker universe from config.toml at startup.
///
/// config.toml structure:
///
///   benchmark = "SPY"
///
///   [[sectors]]
///   ticker = "XLK"
///   name   = "Technology"
///
///   [[industry_groups.XLK]]
///   ticker = "SOXX"
///   name   = "Semiconductors"
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

// ─── TOML deserialization types ───────────────────────────────────────────────

/// Raw entry as it appears in the TOML file.
#[derive(Debug, Deserialize, Clone)]
pub struct TickerEntry {
    pub ticker: String,
    pub name: String,
}

/// Direct mapping of the TOML file schema.
#[derive(Debug, Deserialize)]
struct ConfigFile {
    benchmark: String,
    sectors: Vec<TickerEntry>,
    /// Key = sector ticker (e.g. "XLK"), value = list of industry group entries.
    #[serde(default)]
    industry_groups: HashMap<String, Vec<TickerEntry>>,
}

// ─── Public Config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub benchmark: String,
    /// Ordered list of (ticker, name) for all sectors.
    pub sectors: Vec<(String, String)>,
    /// Map of sector_ticker → Vec<(industry_ticker, industry_name)>.
    /// Preserves insertion order from the TOML file within each sector.
    pub industry_groups: HashMap<String, Vec<(String, String)>>,
}

impl Config {
    /// Load and parse config.toml from the given path.
    /// Call once at startup; store the result in shared AppState.
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
                    "config.toml: [industry_groups.{key}] references unknown sector ticker '{key}'. \
                     Add a matching [[sectors]] entry first."
                );
            }
        }

        let sectors = file
            .sectors
            .into_iter()
            .map(|e| (e.ticker, e.name))
            .collect();

        let industry_groups = file
            .industry_groups
            .into_iter()
            .map(|(sector, entries)| {
                let pairs = entries.into_iter().map(|e| (e.ticker, e.name)).collect();
                (sector, pairs)
            })
            .collect();

        Ok(Self {
            benchmark: file.benchmark,
            sectors,
            industry_groups,
        })
    }

    // ─── Ticker accessors ─────────────────────────────────────────────────────

    /// All sector tickers (excluding benchmark).
    pub fn sector_tickers(&self) -> Vec<&str> {
        self.sectors.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// All industry group tickers across all sectors.
    pub fn industry_tickers(&self) -> Vec<&str> {
        self.industry_groups
            .values()
            .flatten()
            .map(|(t, _)| t.as_str())
            .collect()
    }

    /// Every ticker we need to fetch: benchmark + sectors + industry groups.
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

    /// Returns (ticker, name) pairs for all sectors.
    pub fn sector_pairs(&self) -> Vec<(&str, &str)> {
        self.sectors
            .iter()
            .map(|(t, n)| (t.as_str(), n.as_str()))
            .collect()
    }

    /// Returns (ticker, name) pairs for all industry groups (flattened).
    pub fn industry_pairs(&self) -> Vec<(&str, &str)> {
        self.industry_groups
            .values()
            .flatten()
            .map(|(t, n)| (t.as_str(), n.as_str()))
            .collect()
    }

    /// Returns industry group pairs that belong to a specific sector.
    pub fn industry_pairs_for(&self, sector_ticker: &str) -> Vec<(&str, &str)> {
        self.industry_groups
            .get(sector_ticker)
            .map(|v| v.iter().map(|(t, n)| (t.as_str(), n.as_str())).collect())
            .unwrap_or_default()
    }

    // ─── Lookup helpers ───────────────────────────────────────────────────────

    /// Human-readable name for any ticker (sector or industry group).
    pub fn name_of(&self, ticker: &str) -> Option<&str> {
        self.sectors
            .iter()
            .find(|(t, _)| t == ticker)
            .map(|(_, n)| n.as_str())
            .or_else(|| {
                self.industry_groups
                    .values()
                    .flatten()
                    .find(|(t, _)| t == ticker)
                    .map(|(_, n)| n.as_str())
            })
    }

    /// Returns true if the ticker is a sector (not an industry group).
    pub fn is_sector(&self, ticker: &str) -> bool {
        self.sectors.iter().any(|(t, _)| t == ticker)
    }

    /// Parent sector ticker for a given industry group ticker.
    pub fn parent_sector(&self, industry_ticker: &str) -> Option<&str> {
        self.industry_groups
            .iter()
            .find(|(_, groups)| groups.iter().any(|(t, _)| t == industry_ticker))
            .map(|(sector, _)| sector.as_str())
    }
}
