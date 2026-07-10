use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use reqwest::blocking::Client;
use serde_json::{Map, Value};
use url::Url;

use crate::error::{AppResult, msg};
use crate::models::identity::{ChainAlias, EvmAddress, chain_id_for_alias};
use crate::models::path::RelativePath;
use crate::models::run::RunTarget;
use crate::models::source::{
    ArtifactSourceFile, CompilerMetadata, ContractMetadata, SourceBundle, SourceFile,
    SourceMetadata, SourceProviderMetadata, VerifiedSourceMetadata,
};

#[derive(Clone, Debug)]
pub enum SourceProviderFetch {
    Verified(SourceBundle),
    SourceUnavailable(SourceUnavailableBundle),
}

#[derive(Clone, Debug)]
pub struct SourceUnavailableBundle {
    pub provider_payload: Value,
    pub normalized_payload: VerifiedSourceMetadata,
    pub reason: String,
}

pub fn fetch_verified_source(
    base_url: &Url,
    api_key: Option<&str>,
    headers: &BTreeMap<String, String>,
    address: &EvmAddress,
    chain: &ChainAlias,
) -> AppResult<SourceProviderFetch> {
    let chain_id = chain_id_for_alias(chain)?;
    let endpoint = normalize_api_endpoint(base_url)?;
    let mut url = endpoint.clone();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("module", "contract");
        pairs.append_pair("action", "getsourcecode");
        pairs.append_pair("address", address.as_str());
        pairs.append_pair("chainid", &chain_id.to_string());
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
        if let Some(reason) = source_unavailable_payload_reason(&payload) {
            return Ok(SourceProviderFetch::SourceUnavailable(
                source_unavailable_bundle(
                    payload.clone(),
                    address,
                    chain,
                    chain_id,
                    &endpoint,
                    message,
                    0,
                    None,
                    reason,
                ),
            ));
        }
        return Err(msg(format!(
            "source API returned an unusable payload: status={status:?} message={message:?}"
        )));
    };
    if status != "1" || result.is_empty() {
        if let Some(reason) = source_unavailable_payload_reason(&payload) {
            return Ok(SourceProviderFetch::SourceUnavailable(
                source_unavailable_bundle(
                    payload.clone(),
                    address,
                    chain,
                    chain_id,
                    &endpoint,
                    message,
                    result.len(),
                    None,
                    reason,
                ),
            ));
        }
        return Err(msg(format!(
            "source API returned an unusable payload: status={status:?} message={message:?}"
        )));
    }
    let primary = result
        .first()
        .and_then(Value::as_object)
        .ok_or_else(|| msg("source API returned an unexpected result shape"))?;

    if let Some(reason) = source_unavailable_result_reason(primary) {
        return Ok(SourceProviderFetch::SourceUnavailable(
            source_unavailable_bundle(
                payload.clone(),
                address,
                chain,
                chain_id,
                &endpoint,
                message,
                result.len(),
                Some(primary),
                reason,
            ),
        ));
    }

    let (files, source_layout, source_meta) = parse_source_code_result(primary)?;
    let normalized = normalized_metadata(
        address,
        chain,
        chain_id,
        &endpoint,
        message,
        result.len(),
        primary,
        source_layout,
        source_meta,
        files
            .iter()
            .map(|item| ArtifactSourceFile {
                path: item.path.clone(),
                length: item.content.len(),
                original_path: None,
            })
            .collect(),
    );

    Ok(SourceProviderFetch::Verified(SourceBundle {
        provider_payload: payload,
        normalized_payload: normalized,
        files,
    }))
}

fn normalized_metadata(
    address: &EvmAddress,
    chain: &ChainAlias,
    chain_id: crate::models::identity::ChainId,
    endpoint: &Url,
    message: &str,
    result_count: usize,
    primary: &Map<String, Value>,
    source_layout: String,
    source_meta: SourceMetadata,
    files: Vec<ArtifactSourceFile>,
) -> VerifiedSourceMetadata {
    VerifiedSourceMetadata {
        target: RunTarget {
            address: address.clone(),
            chain: chain.clone(),
            chain_id: Some(chain_id),
        },
        provider: SourceProviderMetadata {
            kind: "etherscan-compatible".to_string(),
            endpoint: endpoint.to_string(),
            message: message.to_string(),
            result_count,
        },
        contract: ContractMetadata {
            name: string_field(primary, "ContractName"),
            file_name: non_empty_string_field(primary, "ContractFileName").map(RelativePath::new),
            proxy: string_field(primary, "Proxy") == "1",
            implementation: parse_optional_address_field(primary, "Implementation"),
            similar_match: non_empty_string_field(primary, "SimilarMatch"),
        },
        compiler: CompilerMetadata {
            version: string_field(primary, "CompilerVersion"),
            kind: string_field(primary, "CompilerType"),
            optimization_used: string_field(primary, "OptimizationUsed"),
            runs: string_field(primary, "Runs"),
            evm_version: string_field(primary, "EVMVersion"),
            constructor_arguments: string_field(primary, "ConstructorArguments"),
            license_type: string_field(primary, "LicenseType"),
            library: string_field(primary, "Library"),
            swarm_source: string_field(primary, "SwarmSource"),
        },
        abi: parse_json_string(primary.get("ABI").and_then(Value::as_str)),
        source_layout,
        source_meta,
        files,
    }
}

fn source_unavailable_bundle(
    provider_payload: Value,
    address: &EvmAddress,
    chain: &ChainAlias,
    chain_id: crate::models::identity::ChainId,
    endpoint: &Url,
    message: &str,
    result_count: usize,
    primary: Option<&Map<String, Value>>,
    reason: String,
) -> SourceUnavailableBundle {
    let empty = Map::new();
    let primary = primary.unwrap_or(&empty);
    SourceUnavailableBundle {
        provider_payload,
        normalized_payload: normalized_metadata(
            address,
            chain,
            chain_id,
            endpoint,
            message,
            result_count,
            primary,
            String::new(),
            SourceMetadata::default(),
            Vec::new(),
        ),
        reason,
    }
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

fn source_unavailable_payload_reason(payload: &Value) -> Option<String> {
    let mut haystack = Vec::new();
    for key in ["message", "result"] {
        if let Some(text) = payload.get(key).and_then(Value::as_str) {
            haystack.push(text);
        }
    }
    source_unavailable_reason_from_text(&haystack.join(" "))
}

fn source_unavailable_result_reason(result: &Map<String, Value>) -> Option<String> {
    let raw_source = string_field(result, "SourceCode");
    let raw_abi = string_field(result, "ABI");
    if raw_source.trim().is_empty() {
        return Some("source API returned no source code for this address".to_string());
    }
    source_unavailable_reason_from_text(&format!("{raw_source} {raw_abi}"))
}

fn source_unavailable_reason_from_text(text: &str) -> Option<String> {
    let lowered = text.to_lowercase();
    if lowered.contains("source code not verified")
        || lowered.contains("contract source code not verified")
        || lowered.contains("source not verified")
        || lowered.contains("not verified")
    {
        return Some("source code is not verified on the provider".to_string());
    }
    if lowered.contains("no records found") || lowered.contains("source code not available") {
        return Some("source API returned no verified source record for this address".to_string());
    }
    None
}

fn normalize_api_endpoint(base_url: &Url) -> AppResult<Url> {
    let mut url = base_url.clone();
    let path = url.path().trim_end_matches('/');
    if path.ends_with("/v2/api") || path.ends_with("/api") {
        return Ok(url);
    }
    let next_path = if path.is_empty() {
        "/v2/api".to_string()
    } else {
        format!("{path}/v2/api")
    };
    url.set_path(&next_path);
    Ok(url)
}

fn parse_source_code_result(
    result: &Map<String, Value>,
) -> AppResult<(Vec<SourceFile>, String, SourceMetadata)> {
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
                    path: RelativePath::new(sanitize_source_path(raw_path)),
                    content,
                });
            }
        }
        if !files.is_empty() {
            let meta = SourceMetadata {
                language: parsed_json
                    .get("language")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                settings: parsed_json
                    .get("settings")
                    .cloned()
                    .unwrap_or(Value::Object(Map::new())),
            };
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
            path: RelativePath::new(sanitize_source_path(&filename)),
            content: raw_source,
        }],
        "flattened".to_string(),
        SourceMetadata {
            language: String::new(),
            settings: Value::Object(Map::new()),
        },
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

fn non_empty_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    let value = string_field(object, key);
    if value.is_empty() { None } else { Some(value) }
}

fn parse_optional_address_field(object: &Map<String, Value>, key: &str) -> Option<EvmAddress> {
    non_empty_string_field(object, key).and_then(|value| EvmAddress::new(value).ok())
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

pub fn merge_unique_lists(groups: &[&[String]]) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut merged = Vec::new();
    for group in groups {
        for entry in *group {
            if seen.insert(entry.as_str()) {
                merged.push(entry.clone());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_source_code_is_source_unavailable() {
        let object = json!({
            "SourceCode": "",
            "ABI": "Contract source code not verified",
            "ContractName": ""
        })
        .as_object()
        .expect("object")
        .clone();

        let reason = source_unavailable_result_reason(&object).expect("unavailable");

        assert!(reason.contains("no source code"));
    }

    #[test]
    fn no_records_payload_is_source_unavailable() {
        let payload = json!({
            "status": "0",
            "message": "NOTOK",
            "result": "No records found"
        });

        let reason = source_unavailable_payload_reason(&payload).expect("unavailable");

        assert!(reason.contains("no verified source record"));
    }
    #[test]
    fn parse_optional_address_field_accepts_valid_implementation() {
        let object = Map::from_iter([(
            "Implementation".to_string(),
            Value::String("0x52908400098527886E0F7030069857D2E4169EE7".to_string()),
        )]);

        let parsed =
            parse_optional_address_field(&object, "Implementation").expect("valid address");
        assert_eq!(
            parsed.as_str(),
            "0x52908400098527886E0F7030069857D2E4169EE7"
        );
    }

    #[test]
    fn parse_optional_address_field_drops_invalid_implementation() {
        let object = Map::from_iter([(
            "Implementation".to_string(),
            Value::String("0x1234567890abcdef1234567890ABCDEF12345678".to_string()),
        )]);

        assert!(parse_optional_address_field(&object, "Implementation").is_none());
    }
}
