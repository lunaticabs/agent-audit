use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::{AppError, AppResult, msg};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChainId(u64);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ChainAlias(String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EvmAddress(String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RunId(String);

impl ChainId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl Default for ChainId {
    fn default() -> Self {
        Self(1)
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for ChainId {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl ChainAlias {
    pub fn new(value: impl AsRef<str>) -> AppResult<Self> {
        let normalized = normalize_chain_alias(value.as_ref());
        if normalized.is_empty() {
            return Err(msg("chain alias must not be empty"));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ChainAlias {
    fn default() -> Self {
        Self("eth".to_string())
    }
}

impl fmt::Display for ChainAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ChainAlias {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ChainAlias {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ChainAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

impl EvmAddress {
    pub fn new(value: impl AsRef<str>) -> AppResult<Self> {
        let raw = value.as_ref().trim();
        validate_evm_address(raw)?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_lowercase(&self) -> String {
        self.0.to_lowercase()
    }

    pub fn zero() -> Self {
        Self("0x0000000000000000000000000000000000000000".to_string())
    }
}

impl fmt::Display for EvmAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for EvmAddress {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Default for EvmAddress {
    fn default() -> Self {
        Self::zero()
    }
}

impl FromStr for EvmAddress {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for EvmAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

impl RunId {
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(msg("run_id must not be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new_unchecked(String::new())
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RunId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for RunId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

pub fn normalize_chain_alias(chain: &str) -> String {
    chain
        .trim()
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

pub fn chain_id_for_alias(alias: &ChainAlias) -> AppResult<ChainId> {
    if let Ok(value) = alias.as_str().parse::<u64>() {
        return Ok(ChainId::new(value));
    }
    chain_aliases()
        .get(alias.as_str())
        .copied()
        .map(ChainId::new)
        .ok_or_else(|| msg(format!("unknown chain alias: {}", alias.as_str())))
}

fn validate_evm_address(value: &str) -> AppResult<()> {
    let Some(body) = value.strip_prefix("0x") else {
        return Err(AppError::InvalidAddress(value.to_string()));
    };
    if body.len() != 40 || !body.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(AppError::InvalidAddress(value.to_string()));
    }

    let has_lower = body.chars().any(|ch| ch.is_ascii_lowercase());
    let has_upper = body.chars().any(|ch| ch.is_ascii_uppercase());
    if has_lower && has_upper && !is_valid_eip55_checksum(value) {
        return Err(AppError::InvalidAddress(value.to_string()));
    }
    Ok(())
}

fn is_valid_eip55_checksum(value: &str) -> bool {
    use sha3::{Digest, Keccak256};

    let Some(body) = value.strip_prefix("0x") else {
        return false;
    };
    let lowercase = body.to_lowercase();
    let digest = Keccak256::digest(lowercase.as_bytes());
    for (index, ch) in body.chars().enumerate() {
        if ch.is_ascii_digit() {
            continue;
        }
        let byte = digest[index / 2];
        let nibble = if index % 2 == 0 {
            (byte >> 4) & 0x0f
        } else {
            byte & 0x0f
        };
        if nibble >= 8 && !ch.is_ascii_uppercase() {
            return false;
        }
        if nibble < 8 && !ch.is_ascii_lowercase() {
            return false;
        }
    }
    true
}

fn chain_aliases() -> &'static std::collections::BTreeMap<&'static str, u64> {
    use std::collections::BTreeMap;
    use std::sync::OnceLock;

    static CELL: OnceLock<BTreeMap<&'static str, u64>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut map = BTreeMap::new();
        for (k, v) in [
            ("eth", 1),
            ("ethereum", 1),
            ("mainnet", 1),
            ("holesky", 17000),
            ("hoodi", 560048),
            ("sepolia", 11155111),
            ("bsc", 56),
            ("bnb", 56),
            ("bnbsmartchain", 56),
            ("binancesmartchain", 56),
            ("bsctestnet", 97),
            ("bnbtestnet", 97),
            ("polygon", 137),
            ("matic", 137),
            ("polygonmainnet", 137),
            ("amoy", 80002),
            ("polygonamoy", 80002),
            ("base", 8453),
            ("basemainnet", 8453),
            ("basesepolia", 84532),
            ("arb", 42161),
            ("arbone", 42161),
            ("arbitrum", 42161),
            ("arbitrumone", 42161),
            ("arbnova", 42170),
            ("arbitrumnova", 42170),
            ("arbsepolia", 421614),
            ("arbitrumsepolia", 421614),
            ("op", 10),
            ("optimism", 10),
            ("opmainnet", 10),
            ("opsepolia", 11155420),
            ("optimismsepolia", 11155420),
            ("avalanche", 43114),
            ("avax", 43114),
            ("avalanchecchain", 43114),
            ("fuji", 43113),
            ("avalanchefuji", 43113),
            ("linea", 59144),
            ("lineasepolia", 59141),
            ("blast", 81457),
            ("blastsepolia", 168587773),
            ("scroll", 534352),
            ("scrollsepolia", 534351),
            ("mantle", 5000),
            ("mantlesepolia", 5003),
            ("gnosis", 100),
            ("xdai", 100),
            ("celo", 42220),
            ("celosepolia", 11142220),
            ("zksync", 324),
            ("zksyncmainnet", 324),
            ("zksyncsepolia", 300),
            ("opbnb", 204),
            ("opbnbtestnet", 5611),
            ("moonbeam", 1284),
            ("moonriver", 1285),
            ("moonbasealpha", 1287),
            ("bittorrent", 199),
            ("btt", 199),
            ("btttestnet", 1029),
            ("fraxtal", 252),
            ("fraxtalhoodi", 2523),
            ("sonic", 146),
            ("sonictestnet", 14601),
            ("sei", 1329),
            ("seitestnet", 1328),
            ("taiko", 167000),
            ("taikohoodi", 167013),
            ("unichain", 130),
            ("unichainsepolia", 1301),
            ("world", 480),
            ("worldchain", 480),
        ] {
            map.insert(k, v);
        }
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_lowercase_address() {
        let address = EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678")
            .expect("valid lowercase address");
        assert_eq!(
            address.as_str(),
            "0x1234567890abcdef1234567890abcdef12345678"
        );
    }

    #[test]
    fn rejects_invalid_checksum_mixed_case_address() {
        let error = EvmAddress::new("0x1234567890abcdef1234567890ABCDEF12345678")
            .expect_err("invalid checksum");
        assert!(matches!(error, AppError::InvalidAddress(_)));
    }

    #[test]
    fn normalizes_chain_alias_input() {
        let chain = ChainAlias::new(" Arbitrum-One ").expect("valid chain alias");
        assert_eq!(chain.as_str(), "arbitrumone");
    }

    #[test]
    fn rejects_empty_run_id() {
        assert!(RunId::new("   ").is_err());
    }

    #[test]
    fn deserializing_invalid_mixed_case_address_fails() {
        let error =
            serde_json::from_str::<EvmAddress>("\"0x1234567890abcdef1234567890ABCDEF12345678\"")
                .expect_err("invalid address should fail");
        assert!(error.to_string().contains("invalid EVM address"));
    }

    #[test]
    fn deserializing_chain_alias_normalizes_input() {
        let alias = serde_json::from_str::<ChainAlias>("\" Arbitrum-One \"")
            .expect("deserialize chain alias");
        assert_eq!(alias.as_str(), "arbitrumone");
    }

    #[test]
    fn deserializing_empty_run_id_fails() {
        let error = serde_json::from_str::<RunId>("\"   \"").expect_err("empty run id should fail");
        assert!(error.to_string().contains("run_id must not be empty"));
    }
}
