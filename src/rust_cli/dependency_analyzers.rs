use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use regex::Regex;
use serde_json::{Value, json};

pub fn analyze_dependencies(bundle_payload: &Value, workspace_root: &Path) -> Vec<Value> {
    let Some(dependencies) = bundle_payload.get("dependencies").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut findings = Vec::new();
    let mut verifier_records = Vec::new();
    for dependency in dependencies {
        if dependency.get("role").and_then(Value::as_str) == Some("verifier") {
            verifier_records.push(dependency.clone());
        } else {
            findings.extend(analyze_dependency_record(dependency, workspace_root));
        }
    }
    if !verifier_records.is_empty() {
        findings.extend(analyze_verifier_group(bundle_payload, &verifier_records, workspace_root));
    }
    findings
}

fn analyze_dependency_record(record: &Value, workspace_root: &Path) -> Vec<Value> {
    let mut findings = Vec::new();
    if let Some(related) = record.get("related_contracts").and_then(Value::as_array) {
        for nested in related {
            findings.extend(analyze_dependency_record(nested, workspace_root));
        }
    }
    findings
}

fn analyze_verifier_group(bundle_payload: &Value, records: &[Value], workspace_root: &Path) -> Vec<Value> {
    let mut findings = Vec::new();
    let expected_arities = extract_expected_verifier_arities(bundle_payload, workspace_root);
    let mut gamma_delta_hits = Vec::new();
    for record in records {
        if record.get("status").and_then(Value::as_str) != Some("fetched") {
            continue;
        }
        for item in analyze_single_verifier_record(record, workspace_root, &expected_arities) {
            if item.get("title").and_then(Value::as_str) == Some("verifier-gamma-delta-equality") {
                gamma_delta_hits.push(item);
            } else {
                findings.push(item);
            }
        }
    }
    if !gamma_delta_hits.is_empty() {
        findings.push(group_gamma_delta_findings(&gamma_delta_hits));
    }
    findings
}

fn analyze_single_verifier_record(
    record: &Value,
    workspace_root: &Path,
    expected_arities: &BTreeMap<String, usize>,
) -> Vec<Value> {
    let mut findings = Vec::new();
    let abi_arity = verifier_abi_pubsignal_arity(record);
    let expected_arity = expected_verifier_arity(record, expected_arities);
    if let (Some(expected_arity), Some(abi_arity)) = (expected_arity, abi_arity) {
        if expected_arity != abi_arity {
            findings.push(json!({
                "title": "verifier-pubsignals-arity-mismatch",
                "severity": "critical",
                "confidence": "high",
                "summary": format!(
                    "Main-contract interface expects {expected_arity} public signals for {}, but the fetched verifier ABI exposes {abi_arity}. This indicates an interface/verifier mismatch on a critical proof-validation path.",
                    record.get("name").and_then(Value::as_str).unwrap_or_default()
                ),
                "source": "dependency-verifier",
                "location": "",
                "evidence_artifacts": verifier_evidence_artifacts(record),
            }));
        }
    }

    if let Some(files) = record.get("files").and_then(Value::as_array) {
        for file_entry in files {
            let Some(relative_path) = file_entry.get("path").and_then(Value::as_str) else {
                continue;
            };
            let source_path = workspace_root.join("sources").join(relative_path);
            if !source_path.exists() {
                continue;
            }
            if let Ok(text) = fs::read_to_string(source_path) {
                findings.extend(analyze_verifier_source_file(record, relative_path, &text));
            }
        }
    }
    findings
}

fn analyze_verifier_source_file(record: &Value, relative_path: &str, source_text: &str) -> Vec<Value> {
    let mut findings = Vec::new();
    let constants = extract_uint_constants(source_text);
    let gamma = [
        "gammax1",
        "gammax2",
        "gammay1",
        "gammay2",
    ]
    .iter()
    .map(|name| constants.get(*name).map(|(value, _)| value.clone()).unwrap_or_default())
    .collect::<Vec<_>>();
    let delta = [
        "deltax1",
        "deltax2",
        "deltay1",
        "deltay2",
    ]
    .iter()
    .map(|name| constants.get(*name).map(|(value, _)| value.clone()).unwrap_or_default())
    .collect::<Vec<_>>();
    if gamma.iter().all(|value| !value.is_empty()) && gamma == delta {
        let location_line = constants.get("gammax1").map(|(_, line)| *line).unwrap_or(1);
        findings.push(json!({
            "title": "verifier-gamma-delta-equality",
            "severity": "critical",
            "confidence": "high",
            "summary": format!(
                "Verifier {} defines identical gamma and delta G2 points. In Groth16-style verifiers this is a high-signal anomaly that can break input binding and should be treated as a critical manual-review item.",
                record.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or_else(|| record.get("address").and_then(Value::as_str).unwrap_or_default())
            ),
            "source": "dependency-verifier",
            "location": format!("{relative_path}:{location_line}"),
            "evidence_artifacts": vec![format!("sources/{relative_path}")],
        }));
    }
    findings
}

fn group_gamma_delta_findings(findings: &[Value]) -> Value {
    let affected = findings
        .iter()
        .filter_map(|item| item.get("location").and_then(Value::as_str).map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    let mut evidence = Vec::new();
    for item in findings {
        if let Some(artifacts) = item.get("evidence_artifacts").and_then(Value::as_array) {
            for artifact in artifacts {
                if let Some(path) = artifact.as_str() {
                    if !evidence.iter().any(|entry| entry == path) {
                        evidence.push(path.to_string());
                    }
                }
            }
        }
    }
    let mut preview = affected.iter().take(4).cloned().collect::<Vec<_>>().join("; ");
    if affected.len() > 4 {
        preview.push_str(&format!("; and {} more", affected.len() - 4));
    }
    json!({
        "title": "verifier-gamma-delta-equality-systemic",
        "severity": "critical",
        "confidence": "high",
        "summary": format!(
            "Multiple fetched Groth16 verifier contracts define identical gamma and delta G2 points, suggesting a systemic proof-binding flaw across the verifier set. Affected locations include {preview}."
        ),
        "source": "dependency-verifier",
        "location": affected.first().cloned().unwrap_or_default(),
        "evidence_artifacts": evidence,
    })
}

fn extract_uint_constants(source_text: &str) -> BTreeMap<String, (String, usize)> {
    let pattern = Regex::new(r"uint(?:256)?\s+constant\s+([A-Za-z_]\w*)\s*=\s*([0-9xa-fA-F]+)\s*;")
        .expect("valid constant regex");
    let mut constants = BTreeMap::new();
    for (index, line) in source_text.lines().enumerate() {
        if let Some(capture) = pattern.captures(line) {
            constants.insert(
                capture.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default(),
                (
                    capture.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default(),
                    index + 1,
                ),
            );
        }
    }
    constants
}

fn extract_expected_verifier_arities(bundle_payload: &Value, workspace_root: &Path) -> BTreeMap<String, usize> {
    let pattern = Regex::new(
        r"interface\s+([A-Za-z_]\w*)\s*\{[^}]*?function\s+verifyProof\s*\([^)]*uint\[(\d+)\]\s+calldata\s+_pubSignals",
    )
    .expect("valid interface regex");
    let mut expected = BTreeMap::new();
    if let Some(files) = bundle_payload.get("files").and_then(Value::as_array) {
        for file_entry in files {
            let Some(relative_path) = file_entry.get("path").and_then(Value::as_str) else {
                continue;
            };
            let path = workspace_root.join("sources").join(relative_path);
            if !path.exists() {
                continue;
            }
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            for capture in pattern.captures_iter(&text) {
                let Some(name) = capture.get(1).map(|m| m.as_str().to_string()) else {
                    continue;
                };
                let Some(arity) = capture.get(2).and_then(|m| m.as_str().parse::<usize>().ok()) else {
                    continue;
                };
                expected.insert(name, arity);
            }
        }
    }
    expected
}

fn verifier_abi_pubsignal_arity(record: &Value) -> Option<usize> {
    let abi = record.get("abi")?.as_array()?;
    let pattern = Regex::new(r"uint(?:256)?\[(\d+)\]").expect("valid arity regex");
    for item in abi {
        if item.get("name").and_then(Value::as_str) != Some("verifyProof") {
            continue;
        }
        let inputs = item.get("inputs")?.as_array()?;
        for arg in inputs {
            let name = arg.get("name").and_then(Value::as_str).unwrap_or_default();
            let internal_type = arg.get("internalType").and_then(Value::as_str).unwrap_or_default();
            if name != "_pubSignals" && !internal_type.contains("pubSignals") {
                continue;
            }
            if let Some(capture) = pattern.captures(internal_type) {
                return capture.get(1).and_then(|m| m.as_str().parse::<usize>().ok());
            }
        }
    }
    None
}

fn expected_verifier_arity(record: &Value, expected: &BTreeMap<String, usize>) -> Option<usize> {
    let discovery = record.get("discovery")?.as_object()?;
    let internal_type = discovery.get("internal_type")?.as_str()?;
    let capture = Regex::new(r"contract\s+([A-Za-z_]\w*)")
        .expect("valid contract regex")
        .captures(internal_type)?;
    expected.get(capture.get(1)?.as_str()).copied()
}

fn verifier_evidence_artifacts(record: &Value) -> Vec<String> {
    let mut artifacts = Vec::new();
    if let Some(files) = record.get("files").and_then(Value::as_array) {
        for file_entry in files {
            if let Some(path) = file_entry.get("path").and_then(Value::as_str) {
                artifacts.push(format!("sources/{path}"));
            }
        }
    }
    artifacts
}
