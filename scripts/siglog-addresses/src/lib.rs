use std::collections::HashSet;

pub const DEFAULT_LOG_URL: &str = "http://65.109.115.133:4500/file/sig0711.log";
pub const DEFAULT_OUTPUT_PATH: &str = "scripts/addresses/addrss.txt";

const SENDER_PREFIX: &str = "Sender :";
const ADDRESS_PREFIX: &str = "0x";
const ADDRESS_HEX_LEN: usize = 40;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExtractError {
    #[error("invalid sender address: {0}")]
    InvalidSenderAddress(String),
    #[error("sender match at line {sender_line} is missing the following address line")]
    MissingAddressAfterSender { sender_line: usize },
    #[error("line {line} after matched sender is not an ethereum address: {value}")]
    InvalidAddressAfterSender { line: usize, value: String },
}

pub fn extract_addresses(log: &str) -> Result<Vec<String>, ExtractError> {
    extract_addresses_matching_sender(log, None)
}

pub fn extract_addresses_for_sender(log: &str, sender: &str) -> Result<Vec<String>, ExtractError> {
    let sender_key = normalize_address(sender)
        .ok_or_else(|| ExtractError::InvalidSenderAddress(sender.to_owned()))?;
    extract_addresses_matching_sender(log, Some(sender_key.as_str()))
}

fn extract_addresses_matching_sender(
    log: &str,
    sender_key: Option<&str>,
) -> Result<Vec<String>, ExtractError> {
    let mut addresses = Vec::new();
    let mut seen = HashSet::new();
    let mut lines = log.lines().enumerate();

    while let Some((line_index, line)) = lines.next() {
        let Some(found_sender) = parse_sender_line(line) else {
            continue;
        };

        if let Some(sender_key) = sender_key {
            if normalize_address(found_sender).as_deref() != Some(sender_key) {
                continue;
            }
        }

        let sender_line = line_index + 1;
        let Some((address_line_index, address_line)) = lines.next() else {
            return Err(ExtractError::MissingAddressAfterSender { sender_line });
        };

        let candidate = address_line.trim();
        if !is_eth_address(candidate) {
            return Err(ExtractError::InvalidAddressAfterSender {
                line: address_line_index + 1,
                value: candidate.to_owned(),
            });
        }

        let candidate_key = normalize_address(candidate).expect("candidate was already validated");
        if seen.insert(candidate_key) {
            addresses.push(candidate.to_owned());
        }
    }

    Ok(addresses)
}

pub fn format_addresses(addresses: &[String]) -> String {
    if addresses.is_empty() {
        String::new()
    } else {
        format!("{}\n", addresses.join("\n"))
    }
}

fn parse_sender_line(line: &str) -> Option<&str> {
    let sender = line.trim().strip_prefix(SENDER_PREFIX)?;
    sender.trim().split_whitespace().next()
}

fn normalize_address(address: &str) -> Option<String> {
    let address = address.trim();
    is_eth_address(address).then(|| address.to_ascii_lowercase())
}

fn is_eth_address(address: &str) -> bool {
    let Some(hex) = address.strip_prefix(ADDRESS_PREFIX) else {
        return false;
    };

    hex.len() == ADDRESS_HEX_LEN && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENDER: &str = "0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0";

    #[test]
    fn extracts_unique_addresses_after_all_senders() {
        let log = "\
Got it :
Sender : 0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0
0x0296057D430a6fB7c0c766ec933110f058eD494d
Has signature : 0x31f57072
Got it :
Sender : 0x1111111111111111111111111111111111111111
0x2222222222222222222222222222222222222222
Has signature : 0xfa461e33
Got it :
Sender : 0x9ed59587af8d7e156707539b9a4a22e7b3cac1a0
0x0296057D430a6fB7c0c766ec933110f058eD494d
Has signature : 0xf04f2707
";

        let addresses = extract_addresses(log).unwrap();

        assert_eq!(
            addresses,
            vec![
                "0x0296057D430a6fB7c0c766ec933110f058eD494d",
                "0x2222222222222222222222222222222222222222",
            ]
        );
    }

    #[test]
    fn extracts_unique_addresses_after_matching_sender() {
        let log = "\
Got it :
Sender : 0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0
0x0296057D430a6fB7c0c766ec933110f058eD494d
Has signature : 0x31f57072
Got it :
Sender : 0x1111111111111111111111111111111111111111
0x2222222222222222222222222222222222222222
Has signature : 0xfa461e33
Got it :
Sender : 0x9ed59587af8d7e156707539b9a4a22e7b3cac1a0
0x0296057D430a6fB7c0c766ec933110f058eD494d
Has signature : 0xf04f2707
Got it :
Sender : 0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0
0x6dDf3986eeDA62146efa7A5DBC6d2F8f9c619A80
Has signature : 0x31f57072
";

        let addresses = extract_addresses_for_sender(log, SENDER).unwrap();

        assert_eq!(
            addresses,
            vec![
                "0x0296057D430a6fB7c0c766ec933110f058eD494d",
                "0x6dDf3986eeDA62146efa7A5DBC6d2F8f9c619A80",
            ]
        );
    }

    #[test]
    fn rejects_invalid_sender_filter_address() {
        let error = extract_addresses_for_sender("", "0xnot-an-address").unwrap_err();

        assert_eq!(
            error,
            ExtractError::InvalidSenderAddress("0xnot-an-address".to_owned())
        );
    }

    #[test]
    fn reports_missing_address_after_sender() {
        let log = "Got it :\nSender : 0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0\n";
        let error = extract_addresses(log).unwrap_err();

        assert_eq!(
            error,
            ExtractError::MissingAddressAfterSender { sender_line: 2 }
        );
    }

    #[test]
    fn reports_invalid_address_after_sender() {
        let log = "\
Got it :
Sender : 0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0
Has signature : 0x31f57072
";
        let error = extract_addresses(log).unwrap_err();

        assert_eq!(
            error,
            ExtractError::InvalidAddressAfterSender {
                line: 3,
                value: "Has signature : 0x31f57072".to_owned(),
            }
        );
    }

    #[test]
    fn formats_like_existing_address_files() {
        let addresses = vec![
            "0x3EEeB3cd20f844a578807fc457388Ceb9A67fAa6".to_owned(),
            "0x91dF0fFc1b95113BA1F41Ca0669FCCAEc0f55876".to_owned(),
        ];

        assert_eq!(
            format_addresses(&addresses),
            "0x3EEeB3cd20f844a578807fc457388Ceb9A67fAa6\n0x91dF0fFc1b95113BA1F41Ca0669FCCAEc0f55876\n"
        );
    }
}
