use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use regex::Regex;
use serde_json::Value;

use crate::models::finding::{DependencyFinding, FindingConfidence, FindingSeverity};
use crate::models::source::{DependencyRecord, SourceBundleArtifact};

pub fn analyze_dependencies(
    bundle: &SourceBundleArtifact,
    workspace_root: &Path,
) -> Vec<DependencyFinding> {
    let verifier_records = collect_verifier_records(&bundle.dependencies);
    if verifier_records.is_empty() {
        return Vec::new();
    }

    analyze_verifier_group(bundle, &verifier_records, workspace_root)
}

fn collect_verifier_records<'a>(records: &'a [DependencyRecord]) -> Vec<&'a DependencyRecord> {
    let mut collected = Vec::new();
    for record in records {
        if record.role == "verifier" {
            collected.push(record);
        }
        collected.extend(collect_verifier_records(&record.related_contracts));
    }
    collected
}

fn analyze_verifier_group(
    bundle: &SourceBundleArtifact,
    records: &[&DependencyRecord],
    workspace_root: &Path,
) -> Vec<DependencyFinding> {
    let mut findings = Vec::new();
    let expected_arities = extract_expected_verifier_arities(bundle, workspace_root);
    let mut gamma_delta_hits = Vec::new();

    for record in records {
        if !record.is_fetched() {
            continue;
        }
        for finding in analyze_single_verifier_record(record, workspace_root, &expected_arities) {
            if finding.title == "verifier-gamma-delta-equality" {
                gamma_delta_hits.push(finding);
            } else {
                findings.push(finding);
            }
        }
    }

    if !gamma_delta_hits.is_empty() {
        findings.push(group_gamma_delta_findings(&gamma_delta_hits));
    }
    findings
}

fn analyze_single_verifier_record(
    record: &DependencyRecord,
    workspace_root: &Path,
    expected_arities: &BTreeMap<String, usize>,
) -> Vec<DependencyFinding> {
    let mut findings = Vec::new();
    let abi_arity = verifier_abi_pubsignal_arity(record);
    let expected_arity = expected_verifier_arity(record, expected_arities);
    if let (Some(expected_arity), Some(abi_arity)) = (expected_arity, abi_arity)
        && expected_arity != abi_arity
    {
        findings.push(
            DependencyFinding::new(
                "verifier-pubsignals-arity-mismatch",
                FindingSeverity::Critical,
                FindingConfidence::High,
                format!(
                    "Main-contract interface expects {expected_arity} public signals for {}, but the fetched verifier ABI exposes {abi_arity}. This indicates an interface/verifier mismatch on a critical proof-validation path.",
                    verifier_display_name(record)
                ),
                "dependency-verifier",
                "",
            )
            .with_evidence(verifier_evidence_artifacts(record)),
        );
    }

    for file_entry in &record.files {
        let relative_path = file_entry.path.as_str();
        let source_path = workspace_root.join("sources").join(relative_path);
        if !source_path.exists() {
            continue;
        }
        if let Ok(text) = fs::read_to_string(source_path) {
            findings.extend(analyze_verifier_source_file(record, relative_path, &text));
        }
    }
    findings
}

fn analyze_verifier_source_file(
    record: &DependencyRecord,
    relative_path: &str,
    source_text: &str,
) -> Vec<DependencyFinding> {
    let mut findings = Vec::new();
    let constants = extract_uint_constants(source_text);
    let gamma = ["gammax1", "gammax2", "gammay1", "gammay2"]
        .iter()
        .map(|name| {
            constants
                .get(*name)
                .map(|(value, _)| value.clone())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let delta = ["deltax1", "deltax2", "deltay1", "deltay2"]
        .iter()
        .map(|name| {
            constants
                .get(*name)
                .map(|(value, _)| value.clone())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    if gamma.iter().all(|value| !value.is_empty()) && gamma == delta {
        let location_line = constants.get("gammax1").map(|(_, line)| *line).unwrap_or(1);
        findings.push(
            DependencyFinding::new(
                "verifier-gamma-delta-equality",
                FindingSeverity::Critical,
                FindingConfidence::High,
                format!(
                    "Verifier {} defines identical gamma and delta G2 points. In Groth16-style verifiers this is a high-signal anomaly that can break input binding and should be treated as a critical manual-review item.",
                    verifier_display_name(record)
                ),
                "dependency-verifier",
                format!("{relative_path}:{location_line}"),
            )
            .with_evidence(vec![format!("sources/{relative_path}")]),
        );
    }
    findings
}

fn group_gamma_delta_findings(findings: &[DependencyFinding]) -> DependencyFinding {
    let affected = findings
        .iter()
        .map(|item| item.location.clone())
        .filter(|location| !location.is_empty())
        .collect::<Vec<_>>();
    let mut evidence = Vec::new();
    for item in findings {
        for artifact in &item.evidence_artifacts {
            if !evidence.iter().any(|entry| entry == artifact) {
                evidence.push(artifact.clone());
            }
        }
    }
    let mut preview = affected
        .iter()
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if affected.len() > 4 {
        preview.push_str(&format!("; and {} more", affected.len() - 4));
    }

    DependencyFinding::new(
        "verifier-gamma-delta-equality-systemic",
        FindingSeverity::Critical,
        FindingConfidence::High,
        format!(
            "Multiple fetched Groth16 verifier contracts define identical gamma and delta G2 points, suggesting a systemic proof-binding flaw across the verifier set. Affected locations include {preview}."
        ),
        "dependency-verifier",
        affected.first().cloned().unwrap_or_default(),
    )
    .with_evidence(evidence)
}

fn extract_uint_constants(source_text: &str) -> BTreeMap<String, (String, usize)> {
    let pattern = Regex::new(r"uint(?:256)?\s+constant\s+([A-Za-z_]\w*)\s*=\s*([0-9xa-fA-F]+)\s*;")
        .expect("valid constant regex");
    let mut constants = BTreeMap::new();
    for (index, line) in source_text.lines().enumerate() {
        if let Some(capture) = pattern.captures(line) {
            constants.insert(
                capture
                    .get(1)
                    .map(|m| m.as_str().to_lowercase())
                    .unwrap_or_default(),
                (
                    capture
                        .get(2)
                        .map(|m| m.as_str().to_lowercase())
                        .unwrap_or_default(),
                    index + 1,
                ),
            );
        }
    }
    constants
}

fn extract_expected_verifier_arities(
    bundle: &SourceBundleArtifact,
    workspace_root: &Path,
) -> BTreeMap<String, usize> {
    let pattern = Regex::new(
        r"interface\s+([A-Za-z_]\w*)\s*\{[^}]*?function\s+verifyProof\s*\([^)]*uint\[(\d+)\]\s+calldata\s+_pubSignals",
    )
    .expect("valid interface regex");
    let mut expected = BTreeMap::new();
    for file_entry in &bundle.files {
        let relative_path = file_entry.path.as_str();
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
            let Some(arity) = capture
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
            else {
                continue;
            };
            expected.insert(name, arity);
        }
    }
    expected
}

fn verifier_abi_pubsignal_arity(record: &DependencyRecord) -> Option<usize> {
    let abi = record.abi.as_array()?;
    let pattern = Regex::new(r"uint(?:256)?\[(\d+)\]").expect("valid arity regex");
    for item in abi {
        if item.get("name").and_then(Value::as_str) != Some("verifyProof") {
            continue;
        }
        let inputs = item.get("inputs")?.as_array()?;
        for arg in inputs {
            let name = arg.get("name").and_then(Value::as_str).unwrap_or_default();
            let internal_type = arg
                .get("internalType")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if name != "_pubSignals" && !internal_type.contains("pubSignals") {
                continue;
            }
            if let Some(capture) = pattern.captures(internal_type) {
                return capture
                    .get(1)
                    .and_then(|m| m.as_str().parse::<usize>().ok());
            }
        }
    }
    None
}

fn expected_verifier_arity(
    record: &DependencyRecord,
    expected: &BTreeMap<String, usize>,
) -> Option<usize> {
    let internal_type = record.discovery.as_ref()?.internal_type.as_str();
    let capture = Regex::new(r"contract\s+([A-Za-z_]\w*)")
        .expect("valid contract regex")
        .captures(internal_type)?;
    expected.get(capture.get(1)?.as_str()).copied()
}

fn verifier_evidence_artifacts(record: &DependencyRecord) -> Vec<String> {
    record
        .files
        .iter()
        .map(|file_entry| format!("sources/{}", file_entry.path))
        .collect()
}

fn verifier_display_name(record: &DependencyRecord) -> &str {
    if !record.name.is_empty() {
        &record.name
    } else {
        record.address.as_str()
    }
}
