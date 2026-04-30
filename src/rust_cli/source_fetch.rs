use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;

use super::errors::{AppResult, msg};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct SourceBundle {
    pub provider_payload: Value,
    pub normalized_payload: Value,
    pub files: Vec<SourceFile>,
}

pub fn fetch_verified_source(
    base_url: &str,
    api_key: Option<&str>,
    headers: &BTreeMap<String, String>,
    address: &str,
    chain: &str,
) -> AppResult<SourceBundle> {
    let chain_id = chain_to_chain_id(chain);
    let endpoint = normalize_api_endpoint(base_url)?;
    let mut url = Url::parse(&endpoint)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("module", "contract");
        pairs.append_pair("action", "getsourcecode");
        pairs.append_pair("address", address);
        pairs.append_pair("chainid", &chain_id);
        if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
            pairs.append_pair("apikey", key);
        }
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut request = client.get(url).header("Accept", "application/json");
    for (key, value) in headers {
        request = request.header(key, value);
    }
    let response = request.send()?;
    let status_code = response.status();
    let text = response.text()?;
    if !status_code.is_success() {
        let body = truncate_chars(&text, 300);
        return Err(msg(format!(
            "source API request failed with HTTP {}: {body}",
            status_code.as_u16()
        )));
    }

    let payload: Value = serde_json::from_str(&text)?;
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = payload.get("result").and_then(Value::as_array);
    let Some(result) = result else {
        return Err(msg(format!(
            "source API returned an unusable payload: status={status:?} message={message:?}"
        )));
    };
    if status != "1" || result.is_empty() {
        return Err(msg(format!(
            "source API returned an unusable payload: status={status:?} message={message:?}"
        )));
    }
    let primary = result
        .first()
        .and_then(Value::as_object)
        .ok_or_else(|| msg("source API returned an unexpected result shape"))?;

    let (files, source_layout, source_meta) = parse_source_code_result(primary)?;
    let normalized = json!({
        "target": {
            "address": address,
            "chain": chain,
            "chain_id": chain_id,
        },
        "provider": {
            "type": "etherscan-compatible",
            "endpoint": endpoint,
            "message": message,
            "result_count": result.len(),
        },
        "contract": {
            "name": string_field(primary, "ContractName"),
            "file_name": string_field(primary, "ContractFileName"),
            "proxy": string_field(primary, "Proxy") == "1",
            "implementation": string_field(primary, "Implementation"),
            "similar_match": string_field(primary, "SimilarMatch"),
        },
        "compiler": {
            "version": string_field(primary, "CompilerVersion"),
            "type": string_field(primary, "CompilerType"),
            "optimization_used": string_field(primary, "OptimizationUsed"),
            "runs": string_field(primary, "Runs"),
            "evm_version": string_field(primary, "EVMVersion"),
            "constructor_arguments": string_field(primary, "ConstructorArguments"),
            "license_type": string_field(primary, "LicenseType"),
            "library": string_field(primary, "Library"),
            "swarm_source": string_field(primary, "SwarmSource"),
        },
        "abi": parse_json_string(primary.get("ABI").and_then(Value::as_str)),
        "source_layout": source_layout,
        "source_meta": source_meta,
        "files": files.iter().map(|item| json!({"path": item.path, "length": item.content.len()})).collect::<Vec<_>>(),
    });

    Ok(SourceBundle {
        provider_payload: payload,
        normalized_payload: normalized,
        files,
    })
}

pub fn parse_json_string(raw: Option<&str>) -> Value {
    let Some(raw) = raw else {
        return Value::Null;
    };
    let text = raw.trim();
    if text.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(text).unwrap_or(Value::Null)
}

pub fn chain_to_chain_id(chain: &str) -> String {
    let normalized = normalize_chain_alias(chain);
    chain_aliases()
        .get(normalized.as_str())
        .copied()
        .unwrap_or(normalized.as_str())
        .to_string()
}

pub fn normalize_chain_alias(chain: &str) -> String {
    chain
        .trim()
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn normalize_api_endpoint(base_url: &str) -> AppResult<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let mut url = Url::parse(trimmed)?;
    let path = url.path().trim_end_matches('/');
    if path.ends_with("/v2/api") || path.ends_with("/api") {
        return Ok(trimmed.to_string());
    }
    let next_path = if path.is_empty() {
        "/v2/api".to_string()
    } else {
        format!("{path}/v2/api")
    };
    url.set_path(&next_path);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn parse_source_code_result(
    result: &Map<String, Value>,
) -> AppResult<(Vec<SourceFile>, String, Value)> {
    let raw_source = string_field(result, "SourceCode");
    let contract_name = {
        let name = string_field(result, "ContractName");
        if name.is_empty() {
            "Contract".to_string()
        } else {
            name
        }
    };
    let contract_file_name = string_field(result, "ContractFileName");

    let mut stripped = raw_source.trim().to_string();
    if stripped.starts_with("{{") && stripped.ends_with("}}") && stripped.len() >= 4 {
        stripped = stripped[1..stripped.len() - 1].trim().to_string();
    }

    let parsed_json = parse_json_string(Some(&stripped));
    if let Some(sources) = parsed_json.get("sources").and_then(Value::as_object) {
        let mut files = Vec::new();
        for (raw_path, source_entry) in sources {
            if let Some(content) = extract_source_content(source_entry) {
                files.push(SourceFile {
                    path: sanitize_source_path(raw_path),
                    content,
                });
            }
        }
        if !files.is_empty() {
            let meta = json!({
                "language": parsed_json.get("language").and_then(Value::as_str).unwrap_or_default(),
                "settings": parsed_json.get("settings").cloned().unwrap_or(Value::Object(Map::new())),
            });
            return Ok((files, "standard-json".to_string(), meta));
        }
    }

    let mut filename = if contract_file_name.is_empty() {
        format!("{contract_name}.sol")
    } else {
        contract_file_name
    };
    let extension = guess_extension(result, &filename);
    if !extension.is_empty() && !filename.ends_with(&extension) {
        filename.push_str(&extension);
    }
    Ok((
        vec![SourceFile {
            path: sanitize_source_path(&filename),
            content: raw_source,
        }],
        "flattened".to_string(),
        Value::Object(Map::new()),
    ))
}

fn extract_source_content(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object
            .get("content")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn guess_extension(result: &Map<String, Value>, filename: &str) -> String {
    let lowered = filename.to_lowercase();
    if lowered.ends_with(".sol") || lowered.ends_with(".vy") || lowered.ends_with(".yul") {
        return String::new();
    }
    let compiler_type = string_field(result, "CompilerType").to_lowercase();
    if compiler_type.contains("vyper") {
        ".vy".to_string()
    } else {
        ".sol".to_string()
    }
}

fn sanitize_source_path(raw_path: &str) -> String {
    let mut parts = Vec::new();
    for part in raw_path.split('/') {
        match part {
            "" | "." | ".." => {}
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        "Contract.sol".to_string()
    } else {
        parts.join("/")
    }
}

fn string_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn chain_aliases() -> &'static BTreeMap<&'static str, &'static str> {
    use std::sync::OnceLock;

    static CELL: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut map = BTreeMap::new();
        for (k, v) in [
            ("1", "1"),
            ("eth", "1"),
            ("ethereum", "1"),
            ("mainnet", "1"),
            ("holesky", "17000"),
            ("17000", "17000"),
            ("hoodi", "560048"),
            ("560048", "560048"),
            ("sepolia", "11155111"),
            ("11155111", "11155111"),
            ("bsc", "56"),
            ("bnb", "56"),
            ("bnbsmartchain", "56"),
            ("binancesmartchain", "56"),
            ("56", "56"),
            ("bsctestnet", "97"),
            ("bnbtestnet", "97"),
            ("97", "97"),
            ("polygon", "137"),
            ("matic", "137"),
            ("polygonmainnet", "137"),
            ("137", "137"),
            ("amoy", "80002"),
            ("polygonamoy", "80002"),
            ("80002", "80002"),
            ("base", "8453"),
            ("basemainnet", "8453"),
            ("8453", "8453"),
            ("basesepolia", "84532"),
            ("84532", "84532"),
            ("arb", "42161"),
            ("arbone", "42161"),
            ("arbitrum", "42161"),
            ("arbitrumone", "42161"),
            ("42161", "42161"),
            ("arbnova", "42170"),
            ("arbitrumnova", "42170"),
            ("42170", "42170"),
            ("arbsepolia", "421614"),
            ("arbitrumsepolia", "421614"),
            ("421614", "421614"),
            ("op", "10"),
            ("optimism", "10"),
            ("opmainnet", "10"),
            ("10", "10"),
            ("opsepolia", "11155420"),
            ("optimismsepolia", "11155420"),
            ("11155420", "11155420"),
            ("avalanche", "43114"),
            ("avax", "43114"),
            ("avalanchecchain", "43114"),
            ("43114", "43114"),
            ("fuji", "43113"),
            ("avalanchefuji", "43113"),
            ("43113", "43113"),
            ("linea", "59144"),
            ("59144", "59144"),
            ("lineasepolia", "59141"),
            ("59141", "59141"),
            ("blast", "81457"),
            ("81457", "81457"),
            ("blastsepolia", "168587773"),
            ("168587773", "168587773"),
            ("scroll", "534352"),
            ("534352", "534352"),
            ("scrollsepolia", "534351"),
            ("534351", "534351"),
            ("mantle", "5000"),
            ("5000", "5000"),
            ("mantlesepolia", "5003"),
            ("5003", "5003"),
            ("gnosis", "100"),
            ("xdai", "100"),
            ("100", "100"),
            ("celo", "42220"),
            ("42220", "42220"),
            ("celosepolia", "11142220"),
            ("11142220", "11142220"),
            ("zksync", "324"),
            ("zksyncmainnet", "324"),
            ("324", "324"),
            ("zksyncsepolia", "300"),
            ("300", "300"),
            ("opbnb", "204"),
            ("204", "204"),
            ("opbnbtestnet", "5611"),
            ("5611", "5611"),
            ("moonbeam", "1284"),
            ("1284", "1284"),
            ("moonriver", "1285"),
            ("1285", "1285"),
            ("moonbasealpha", "1287"),
            ("1287", "1287"),
            ("bittorrent", "199"),
            ("btt", "199"),
            ("199", "199"),
            ("btttestnet", "1029"),
            ("1029", "1029"),
            ("fraxtal", "252"),
            ("252", "252"),
            ("fraxtalhoodi", "2523"),
            ("2523", "2523"),
            ("sonic", "146"),
            ("146", "146"),
            ("sonictestnet", "14601"),
            ("14601", "14601"),
            ("sei", "1329"),
            ("1329", "1329"),
            ("seitestnet", "1328"),
            ("1328", "1328"),
            ("taiko", "167000"),
            ("167000", "167000"),
            ("taikohoodi", "167013"),
            ("167013", "167013"),
            ("unichain", "130"),
            ("130", "130"),
            ("unichainsepolia", "1301"),
            ("1301", "1301"),
            ("world", "480"),
            ("worldchain", "480"),
            ("480", "480"),
            ("worldsepolia", "4801"),
            ("4801", "4801"),
            ("xdc", "50"),
            ("50", "50"),
            ("xdcapothem", "51"),
            ("51", "51"),
            ("abstract", "2741"),
            ("2741", "2741"),
            ("abstractsepolia", "11124"),
            ("11124", "11124"),
        ] {
            map.insert(k, v);
        }
        map
    })
}

pub fn sanitize_dependency_name(name: &str) -> String {
    let cleaned = name
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "dependency".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn extract_semver(compiler_version: &str) -> String {
    Regex::new(r"(\d+\.\d+\.\d+)")
        .ok()
        .and_then(|re| re.captures(compiler_version))
        .and_then(|caps| caps.get(1))
        .map(|value| value.as_str().to_string())
        .unwrap_or_default()
}

pub fn merge_unique_lists(groups: &[Vec<String>]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    for group in groups {
        for entry in group {
            if seen.insert(entry.clone()) {
                merged.push(entry.clone());
            }
        }
    }
    merged
}
