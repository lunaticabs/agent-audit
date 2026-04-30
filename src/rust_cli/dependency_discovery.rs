use std::collections::BTreeMap;

use regex::Regex;
use serde_json::{Value, json};

pub fn discover_dependencies(bundle_payload: &Value, sources: &BTreeMap<String, String>) -> Value {
    let constructor_candidates = discover_constructor_dependencies(bundle_payload);
    let constant_candidates = discover_source_constant_dependencies(sources);
    let merged = merge_dependency_candidates(&[constructor_candidates.clone(), constant_candidates.clone()]);
    json!({
        "constructor_candidates": constructor_candidates,
        "constant_candidates": constant_candidates,
        "merged_candidates": merged,
    })
}

fn discover_constructor_dependencies(bundle_payload: &Value) -> Vec<Value> {
    let Some(abi) = bundle_payload.get("abi").and_then(Value::as_array) else {
        return Vec::new();
    };
    let constructor = abi
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("constructor"));
    let Some(constructor) = constructor else {
        return Vec::new();
    };
    let Some(inputs) = constructor.get("inputs").and_then(Value::as_array) else {
        return Vec::new();
    };
    let constructor_args = bundle_payload
        .get("compiler")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("constructor_arguments"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let decoded = decode_static_constructor_arguments(inputs, constructor_args);
    decoded
        .into_iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("address"))
        .filter_map(|item| {
            let address = item.get("value").and_then(Value::as_str)?.to_lowercase();
            if !is_valid_address(&address) {
                return None;
            }
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            let internal_type = item.get("internal_type").and_then(Value::as_str).unwrap_or_default();
            let solidity_type = item.get("solidity_type").and_then(Value::as_str).unwrap_or_default();
            Some(json!({
                "address": address,
                "name": name,
                "role": classify_dependency_role(name, internal_type),
                "source": "constructor",
                "internal_type": internal_type,
                "solidity_type": solidity_type,
            }))
        })
        .collect()
}

fn decode_static_constructor_arguments(inputs: &[Value], constructor_args_hex: &str) -> Vec<Value> {
    let hex_body = constructor_args_hex.trim().trim_start_matches("0x");
    if hex_body.is_empty() {
        return Vec::new();
    }
    let mut decoded = Vec::new();
    let mut offset = 0usize;
    for item in inputs {
        let Some(item) = item.as_object() else {
            continue;
        };
        let solidity_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if !is_static_abi_type(solidity_type) {
            break;
        }
        if offset + 64 > hex_body.len() {
            break;
        }
        let word = &hex_body[offset..offset + 64];
        offset += 64;
        decoded.push(json!({
            "name": item.get("name").and_then(Value::as_str).unwrap_or_default(),
            "internal_type": item.get("internalType").and_then(Value::as_str).unwrap_or_default(),
            "solidity_type": solidity_type,
            "type": classify_abi_type(solidity_type),
            "value": decode_static_word(solidity_type, word),
        }));
    }
    decoded
}

fn discover_source_constant_dependencies(sources: &BTreeMap<String, String>) -> Vec<Value> {
    let pattern = Regex::new(
        r"address(?:\s+payable)?(?:\s+(?:public|private|internal|external|constant|immutable))*\s+(?P<name>[A-Za-z_]\w*)\s*=\s*(?:address\s*\()?(?P<addr>0x[a-fA-F0-9]{40})(?:\))?",
    )
    .expect("valid source dependency regex");
    let mut candidates = Vec::new();
    for (path, source) in sources {
        for capture in pattern.captures_iter(source) {
            let address = capture.name("addr").map(|m| m.as_str().to_lowercase()).unwrap_or_default();
            let name = capture.name("name").map(|m| m.as_str()).unwrap_or_default();
            if !is_valid_address(&address) {
                continue;
            }
            candidates.push(json!({
                "address": address,
                "name": name,
                "role": classify_dependency_role(name, ""),
                "source": "source_constant",
                "file": path,
            }));
        }
    }
    candidates
}

fn merge_dependency_candidates(groups: &[Vec<Value>]) -> Vec<Value> {
    let mut merged = Vec::new();
    let mut by_address: BTreeMap<String, usize> = BTreeMap::new();

    for group in groups {
        for item in group {
            let Some(address) = item.get("address").and_then(Value::as_str).map(|s| s.to_lowercase()) else {
                continue;
            };
            if !is_valid_address(&address) {
                continue;
            }
            if let Some(index) = by_address.get(&address).copied() {
                if let Some(existing) = merged.get_mut(index).and_then(Value::as_object_mut) {
                    let sources = existing.entry("sources").or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(array) = sources.as_array_mut() {
                        let source_name = item.get("source").and_then(Value::as_str).unwrap_or("unknown");
                        if !array.iter().any(|entry| entry.as_str() == Some(source_name)) {
                            array.push(Value::String(source_name.to_string()));
                        }
                    }
                    if existing.get("name").and_then(Value::as_str).unwrap_or_default().is_empty() {
                        if let Some(name) = item.get("name").cloned() {
                            existing.insert("name".to_string(), name);
                        }
                    }
                    let role = existing.get("role").and_then(Value::as_str).unwrap_or_default();
                    if matches!(role, "dependency" | "unknown") {
                        if let Some(next_role) = item.get("role").cloned() {
                            existing.insert("role".to_string(), next_role);
                        }
                    }
                    if existing.get("internal_type").is_none() {
                        if let Some(value) = item.get("internal_type").cloned() {
                            existing.insert("internal_type".to_string(), value);
                        }
                    }
                    if existing.get("file").is_none() {
                        if let Some(value) = item.get("file").cloned() {
                            existing.insert("file".to_string(), value);
                        }
                    }
                }
                continue;
            }

            let mut clone = item.clone();
            if let Some(object) = clone.as_object_mut() {
                object.insert("address".to_string(), Value::String(address.clone()));
                object.insert(
                    "sources".to_string(),
                    Value::Array(vec![Value::String(
                        item.get("source").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                    )]),
                );
            }
            by_address.insert(address, merged.len());
            merged.push(clone);
        }
    }
    merged
}

fn classify_dependency_role(name: &str, internal_type: &str) -> &'static str {
    let haystack = format!("{name} {internal_type}").replace('_', "").to_lowercase();
    for (needle, role) in [
        ("verifyproof", "verifier"),
        ("withdraw", "verifier"),
        ("cancel", "verifier"),
        ("update", "verifier"),
        ("proof", "verifier"),
        ("router", "router"),
        ("swap", "router"),
        ("uniswap", "router"),
        ("token", "token"),
        ("erc20", "token"),
        ("oracle", "oracle"),
        ("price", "oracle"),
        ("aggregator", "oracle"),
        ("admin", "access-control"),
        ("owner", "access-control"),
        ("govern", "access-control"),
        ("controller", "controller"),
        ("vault", "vault"),
        ("pool", "pool"),
        ("bridge", "bridge"),
        ("messenger", "bridge"),
        ("weth", "token"),
    ] {
        if haystack.contains(needle) {
            return role;
        }
    }
    if internal_type.to_lowercase().contains("contract ") {
        "contract-dependency"
    } else {
        "dependency"
    }
}

fn is_static_abi_type(solidity_type: &str) -> bool {
    !solidity_type.is_empty()
        && !solidity_type.ends_with(']')
        && solidity_type != "string"
        && solidity_type != "bytes"
}

fn classify_abi_type(solidity_type: &str) -> &'static str {
    let lowered = solidity_type.to_lowercase();
    if lowered == "address" {
        "address"
    } else if lowered == "bool" {
        "bool"
    } else if lowered.starts_with("uint") || lowered.starts_with("int") {
        "int"
    } else if lowered.starts_with("bytes") {
        "bytes"
    } else {
        "word"
    }
}

fn decode_static_word(solidity_type: &str, word_hex: &str) -> String {
    let lowered = solidity_type.to_lowercase();
    if lowered == "address" {
        format!("0x{}", &word_hex[word_hex.len().saturating_sub(40)..].to_lowercase())
    } else if lowered == "bool" {
        let value = u128::from_str_radix(word_hex, 16).unwrap_or(0);
        if value == 0 { "false".to_string() } else { "true".to_string() }
    } else if lowered.starts_with("uint") || lowered.starts_with("int") {
        u128::from_str_radix(word_hex, 16).map(|v| v.to_string()).unwrap_or_else(|_| "0".to_string())
    } else {
        format!("0x{}", word_hex.to_lowercase())
    }
}

pub fn is_valid_address(value: &str) -> bool {
    Regex::new(r"^0x[a-fA-F0-9]{40}$")
        .expect("valid address regex")
        .is_match(value)
}
