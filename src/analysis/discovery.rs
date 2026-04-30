use std::collections::BTreeMap;

use regex::Regex;
use serde_json::Value;

use crate::models::discovery::{
    DependencyCandidate, DependencyCandidateSource, DependencyDiscoveryReport,
};
use crate::models::identity::EvmAddress;
use crate::models::path::RelativePath;
use crate::models::source::VerifiedSourceMetadata;

pub fn discover_dependencies(
    metadata: &VerifiedSourceMetadata,
    sources: &BTreeMap<String, String>,
) -> DependencyDiscoveryReport {
    let constructor_candidates = discover_constructor_dependencies(metadata);
    let constant_candidates = discover_source_constant_dependencies(sources);
    let cast_constant_candidates = discover_source_cast_constant_dependencies(sources);
    let immutable_candidates = discover_immutable_constructor_dependencies(metadata, sources);
    let merged_candidates = merge_dependency_candidates(&[
        constructor_candidates.clone(),
        constant_candidates.clone(),
        cast_constant_candidates.clone(),
        immutable_candidates.clone(),
    ]);

    DependencyDiscoveryReport {
        constructor_candidates,
        constant_candidates,
        cast_constant_candidates,
        immutable_candidates,
        merged_candidates,
    }
}

fn discover_constructor_dependencies(
    metadata: &VerifiedSourceMetadata,
) -> Vec<DependencyCandidate> {
    let Some(abi) = metadata.abi.as_array() else {
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
    let decoded =
        decode_static_constructor_arguments(inputs, &metadata.compiler.constructor_arguments);

    decoded
        .into_iter()
        .filter(|item| item.value_type == DecodedValueType::Address)
        .filter_map(|item| {
            let address = EvmAddress::new(&item.value).ok()?;
            let name = item.name;
            let internal_type = item.internal_type;
            let solidity_type = item.solidity_type;
            Some(DependencyCandidate {
                address,
                name: name.clone(),
                role: classify_dependency_role(&name, &internal_type).to_string(),
                source: Some(DependencyCandidateSource::Constructor),
                sources: Vec::new(),
                internal_type,
                solidity_type,
                declared_type: String::new(),
                file: None,
            })
        })
        .collect()
}

fn decode_static_constructor_arguments(
    inputs: &[Value],
    constructor_args_hex: &str,
) -> Vec<DecodedConstructorArgument> {
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
        decoded.push(DecodedConstructorArgument {
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            internal_type: item
                .get("internalType")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            solidity_type: solidity_type.to_string(),
            value_type: classify_abi_type(solidity_type),
            value: decode_static_word(solidity_type, word),
        });
    }
    decoded
}

fn discover_source_constant_dependencies(
    sources: &BTreeMap<String, String>,
) -> Vec<DependencyCandidate> {
    let pattern = Regex::new(
        r"address(?:\s+payable)?(?:\s+(?:public|private|internal|external|constant|immutable))*\s+(?P<name>[A-Za-z_]\w*)\s*=\s*(?:address\s*\()?(?P<addr>0x[a-fA-F0-9]{40})(?:\))?",
    )
    .expect("valid source dependency regex");
    let mut candidates = Vec::new();
    for (path, source) in sources {
        for capture in pattern.captures_iter(source) {
            let address = capture
                .name("addr")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let name = capture
                .name("name")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let Ok(address) = EvmAddress::new(&address) else {
                continue;
            };
            candidates.push(DependencyCandidate {
                address,
                name: name.clone(),
                role: classify_dependency_role(&name, "").to_string(),
                source: Some(DependencyCandidateSource::SourceConstant),
                sources: Vec::new(),
                internal_type: String::new(),
                solidity_type: String::new(),
                declared_type: String::new(),
                file: Some(RelativePath::new(path)),
            });
        }
    }
    candidates
}

fn discover_source_cast_constant_dependencies(
    sources: &BTreeMap<String, String>,
) -> Vec<DependencyCandidate> {
    let pattern = Regex::new(
        r"(?P<type>[A-Za-z_]\w*)\s+(?:public|private|internal|external|constant|immutable|\s)*\s*(?P<name>[A-Za-z_]\w*)\s*=\s*(?P<cast>[A-Za-z_]\w*)\s*\(\s*(?P<addr>0x[a-fA-F0-9]{40})\s*\)",
    )
    .expect("valid cast dependency regex");
    let mut candidates = Vec::new();
    for (path, source) in sources {
        for capture in pattern.captures_iter(source) {
            let address = capture
                .name("addr")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let Ok(address) = EvmAddress::new(&address) else {
                continue;
            };
            let name = capture
                .name("name")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let cast_type = capture.name("cast").map(|m| m.as_str()).unwrap_or_default();
            let declared_type = capture.name("type").map(|m| m.as_str()).unwrap_or_default();
            let internal_type = if cast_type.is_empty() {
                declared_type.to_string()
            } else {
                format!("contract {cast_type}")
            };
            candidates.push(DependencyCandidate {
                address,
                name: name.clone(),
                role: classify_dependency_role(&name, &internal_type).to_string(),
                source: Some(DependencyCandidateSource::SourceCastConstant),
                sources: Vec::new(),
                internal_type,
                solidity_type: String::new(),
                declared_type: declared_type.to_string(),
                file: Some(RelativePath::new(path)),
            });
        }
    }
    candidates
}

fn discover_immutable_constructor_dependencies(
    metadata: &VerifiedSourceMetadata,
    sources: &BTreeMap<String, String>,
) -> Vec<DependencyCandidate> {
    let immutable_decl = Regex::new(
        r"address(?:\s+payable)?(?P<mods>(?:\s+(?:public|private|internal|external|immutable))+)\s+(?P<name>[A-Za-z_]\w*)\s*;",
    )
    .expect("valid immutable dependency regex");
    let constructor_block =
        Regex::new(r"constructor\s*\((?P<params>[^)]*)\)\s*[^{};]*\{(?P<body>[\s\S]*?)\}")
            .expect("valid constructor block regex");
    let address_param = Regex::new(
        r"(?:address(?:\s+payable)?|[A-Za-z_]\w+)\s+(?:memory\s+|calldata\s+|storage\s+)?(?P<name>[A-Za-z_]\w*)",
    )
    .expect("valid constructor param regex");
    let assignment =
        Regex::new(r"(?P<lhs>[A-Za-z_]\w*)\s*=\s*(?P<rhs>[A-Za-z_]\w*|0x[a-fA-F0-9]{40})\s*;")
            .expect("valid immutable assignment regex");

    let mut candidates = Vec::new();
    let constructor_address_args = decoded_constructor_address_args(metadata);
    for (path, source) in sources {
        let immutable_fields = immutable_decl
            .captures_iter(source)
            .filter(|capture| {
                capture
                    .name("mods")
                    .map(|m| m.as_str().contains("immutable"))
                    .unwrap_or(false)
            })
            .filter_map(|capture| capture.name("name").map(|m| m.as_str().to_string()))
            .collect::<Vec<_>>();
        if immutable_fields.is_empty() {
            continue;
        }
        let Some(constructor) = constructor_block.captures(source) else {
            continue;
        };
        let params_text = constructor
            .name("params")
            .map(|m| m.as_str())
            .unwrap_or_default();
        let body_text = constructor
            .name("body")
            .map(|m| m.as_str())
            .unwrap_or_default();
        let address_params = address_param
            .captures_iter(params_text)
            .filter_map(|capture| capture.name("name").map(|m| m.as_str().to_string()))
            .collect::<Vec<_>>();
        for capture in assignment.captures_iter(body_text) {
            let lhs = capture.name("lhs").map(|m| m.as_str()).unwrap_or_default();
            if !immutable_fields.iter().any(|field| field == lhs) {
                continue;
            }
            let rhs = capture.name("rhs").map(|m| m.as_str()).unwrap_or_default();
            let address = if rhs.starts_with("0x") {
                EvmAddress::new(rhs).ok()
            } else if address_params.iter().any(|param| param == rhs) {
                constructor_address_args.get(rhs).cloned()
            } else {
                continue;
            };
            let Some(address) = address else {
                continue;
            };
            candidates.push(DependencyCandidate {
                address,
                name: lhs.to_string(),
                role: classify_dependency_role(lhs, "").to_string(),
                source: Some(DependencyCandidateSource::ImmutableConstructorAssignment),
                sources: Vec::new(),
                internal_type: String::new(),
                solidity_type: String::new(),
                declared_type: String::new(),
                file: Some(RelativePath::new(path)),
            });
        }
    }
    candidates
}

fn decoded_constructor_address_args(
    metadata: &VerifiedSourceMetadata,
) -> BTreeMap<String, EvmAddress> {
    let Some(abi) = metadata.abi.as_array() else {
        return BTreeMap::new();
    };
    let Some(constructor) = abi
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("constructor"))
    else {
        return BTreeMap::new();
    };
    let Some(inputs) = constructor.get("inputs").and_then(Value::as_array) else {
        return BTreeMap::new();
    };
    decode_static_constructor_arguments(inputs, &metadata.compiler.constructor_arguments)
        .into_iter()
        .filter(|item| item.value_type == DecodedValueType::Address)
        .filter_map(|item| Some((item.name, EvmAddress::new(&item.value).ok()?)))
        .collect()
}

fn merge_dependency_candidates(groups: &[Vec<DependencyCandidate>]) -> Vec<DependencyCandidate> {
    let mut merged: Vec<DependencyCandidate> = Vec::new();
    let mut by_address: BTreeMap<String, usize> = BTreeMap::new();

    for group in groups {
        for item in group {
            let address = item.address.as_lowercase();
            if let Some(index) = by_address.get(&address).copied() {
                if let Some(existing) = merged.get_mut(index) {
                    if let Some(source) = item.source
                        && !existing.sources.contains(&source)
                    {
                        existing.sources.push(source);
                    }
                    if existing.name.is_empty() {
                        existing.name = item.name.clone();
                    }
                    if matches!(existing.role.as_str(), "dependency" | "unknown") {
                        existing.role = item.role.clone();
                    }
                    if existing.internal_type.is_empty() {
                        existing.internal_type = item.internal_type.clone();
                    }
                    if existing.solidity_type.is_empty() {
                        existing.solidity_type = item.solidity_type.clone();
                    }
                    if existing.declared_type.is_empty() {
                        existing.declared_type = item.declared_type.clone();
                    }
                    if existing.file.is_none() {
                        existing.file = item.file.clone();
                    }
                }
                continue;
            }

            let mut candidate = item.clone();
            candidate.address = item.address.clone();
            candidate.sources = item.source.into_iter().collect();
            by_address.insert(address, merged.len());
            merged.push(candidate);
        }
    }
    merged
}

fn classify_dependency_role(name: &str, internal_type: &str) -> &'static str {
    let haystack = format!("{name} {internal_type}")
        .replace('_', "")
        .to_lowercase();
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
        ("registry", "registry"),
        ("factory", "factory"),
        ("adapter", "adapter"),
        ("strategy", "strategy"),
        ("module", "module"),
        ("executor", "module"),
        ("endpoint", "bridge"),
        ("gateway", "bridge"),
        ("admin", "access-control"),
        ("owner", "access-control"),
        ("guardian", "access-control"),
        ("timelock", "access-control"),
        ("safe", "access-control"),
        ("multisig", "access-control"),
        ("govern", "access-control"),
        ("controller", "controller"),
        ("vault", "vault"),
        ("lendingpool", "lending-pool"),
        ("aave", "lending-pool"),
        ("pool", "pool"),
        ("escrow", "escrow"),
        ("staking", "staking"),
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

fn classify_abi_type(solidity_type: &str) -> DecodedValueType {
    let lowered = solidity_type.to_lowercase();
    if lowered == "address" {
        DecodedValueType::Address
    } else if lowered == "bool" {
        DecodedValueType::Bool
    } else if lowered.starts_with("uint") || lowered.starts_with("int") {
        DecodedValueType::Int
    } else if lowered.starts_with("bytes") {
        DecodedValueType::Bytes
    } else {
        DecodedValueType::Word
    }
}

fn decode_static_word(solidity_type: &str, word_hex: &str) -> String {
    let lowered = solidity_type.to_lowercase();
    if lowered == "address" {
        format!(
            "0x{}",
            &word_hex[word_hex.len().saturating_sub(40)..].to_lowercase()
        )
    } else if lowered == "bool" {
        let value = u128::from_str_radix(word_hex, 16).unwrap_or(0);
        if value == 0 {
            "false".to_string()
        } else {
            "true".to_string()
        }
    } else if lowered.starts_with("uint") || lowered.starts_with("int") {
        u128::from_str_radix(word_hex, 16)
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "0".to_string())
    } else {
        format!("0x{}", word_hex.to_lowercase())
    }
}

#[derive(Clone, Debug)]
struct DecodedConstructorArgument {
    name: String,
    internal_type: String,
    solidity_type: String,
    value_type: DecodedValueType,
    value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecodedValueType {
    Address,
    Bool,
    Int,
    Bytes,
    Word,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::run::RunTarget;
    use crate::models::source::{CompilerMetadata, SourceMetadata};
    use serde_json::Value;

    #[test]
    fn discovers_cast_constant_dependencies() {
        let mut sources = BTreeMap::new();
        sources.insert(
            "Main.sol".to_string(),
            r#"
            interface IOracle {}
            contract Main {
                IOracle public oracle = IOracle(0x52908400098527886E0F7030069857D2E4169EE7);
            }
            "#
            .to_string(),
        );

        let result = discover_source_cast_constant_dependencies(&sources);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].address.as_str(),
            "0x52908400098527886E0F7030069857D2E4169EE7"
        );
        assert_eq!(result[0].role, "oracle");
    }

    #[test]
    fn discovers_immutable_constant_constructor_assignment() {
        let mut sources = BTreeMap::new();
        sources.insert(
            "Main.sol".to_string(),
            r#"
            contract Main {
                address public immutable gateway;
                constructor() {
                    gateway = 0x1111111111111111111111111111111111111111;
                }
            }
            "#
            .to_string(),
        );

        let result =
            discover_immutable_constructor_dependencies(&test_metadata(Value::Null, ""), &sources);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].address.as_str(),
            "0x1111111111111111111111111111111111111111"
        );
        assert_eq!(result[0].name, "gateway");
    }

    #[test]
    fn discovers_immutable_constructor_param_assignment() {
        let mut sources = BTreeMap::new();
        sources.insert(
            "Main.sol".to_string(),
            r#"
            contract Main {
                address immutable oracle;
                constructor(address _oracle, string memory label) {
                    oracle = _oracle;
                }
            }
            "#
            .to_string(),
        );

        let metadata = test_metadata(
            serde_json::from_str(
                r#"[{
                    "type": "constructor",
                    "inputs": [
                        {"name": "_oracle", "type": "address", "internalType": "address"},
                        {"name": "label", "type": "string", "internalType": "string"}
                    ]
                }]"#,
            )
            .expect("valid abi fixture"),
            "0000000000000000000000002222222222222222222222222222222222222222",
        );
        let result = discover_immutable_constructor_dependencies(&metadata, &sources);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].address.as_str(),
            "0x2222222222222222222222222222222222222222"
        );
        assert_eq!(result[0].name, "oracle");
    }

    #[test]
    fn extends_role_classification_keywords() {
        assert_eq!(classify_dependency_role("lendingPool", ""), "lending-pool");
        assert_eq!(classify_dependency_role("moduleRegistry", ""), "registry");
        assert_eq!(classify_dependency_role("safe", ""), "access-control");
    }

    fn test_metadata(abi: Value, constructor_arguments: &str) -> VerifiedSourceMetadata {
        VerifiedSourceMetadata {
            target: RunTarget::default(),
            compiler: CompilerMetadata {
                constructor_arguments: constructor_arguments.to_string(),
                ..CompilerMetadata::default()
            },
            abi,
            source_meta: SourceMetadata::default(),
            ..VerifiedSourceMetadata::default()
        }
    }
}
