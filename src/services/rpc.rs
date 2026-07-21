use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};
use url::Url;

pub fn latest_block_number(url: &Url) -> Result<String, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(url.clone())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1u64,
            "method": "eth_blockNumber",
            "params": [],
        }))
        .send()
        .map_err(|error| error.to_string())?;
    let payload: Value = response.json().map_err(|error| error.to_string())?;
    if let Some(error) = payload.get("error") {
        return Err(error.to_string());
    }
    let raw = payload
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| "JSON-RPC response for eth_blockNumber is missing result".to_string())?;
    parse_hex_quantity_to_decimal(raw)
}

fn parse_hex_quantity_to_decimal(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if hex.is_empty() {
        return Err("empty hex quantity".to_string());
    }
    u64::from_str_radix(hex, 16)
        .map(|number| number.to_string())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_quantity_to_decimal_handles_rpc_block_number() {
        assert_eq!(parse_hex_quantity_to_decimal("0x10").expect("parse"), "16");
        assert_eq!(parse_hex_quantity_to_decimal("0x0").expect("parse"), "0");
    }
}
