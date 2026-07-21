use std::{error::Error, fs, path::PathBuf};

use clap::Parser;
use siglog_addresses::{
    DEFAULT_LOG_URL, DEFAULT_OUTPUT_PATH, extract_addresses, extract_addresses_for_sender,
    format_addresses,
};

#[derive(Debug, Parser)]
#[command(
    name = "siglog-addresses",
    about = "Extract addresses from the line after matching Sender entries in a sig log"
)]
struct Args {
    /// Optional Sender address filter.
    #[arg(long)]
    sender: Option<String>,

    /// Log URL or local log file path.
    #[arg(short, long, default_value = DEFAULT_LOG_URL)]
    source: String,

    /// Output file path.
    #[arg(short, long, default_value = DEFAULT_OUTPUT_PATH)]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let log = read_source(&args.source)?;
    let addresses = match args.sender.as_deref() {
        Some(sender) => extract_addresses_for_sender(&log, sender)?,
        None => extract_addresses(&log)?,
    };
    write_output(&args.output, &format_addresses(&addresses))?;

    eprintln!(
        "wrote {} addresses to {}",
        addresses.len(),
        args.output.display()
    );

    Ok(())
}

fn read_source(source: &str) -> Result<String, Box<dyn Error>> {
    if is_http_url(source) {
        let response = reqwest::blocking::get(source)?.error_for_status()?;
        Ok(response.text()?)
    } else {
        Ok(fs::read_to_string(source)?)
    }
}

fn write_output(path: &PathBuf, contents: &str) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, contents)?;
    Ok(())
}

fn is_http_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}
