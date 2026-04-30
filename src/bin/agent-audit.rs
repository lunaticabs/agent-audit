#[path = "../rust_cli/mod.rs"]
mod rust_cli;

fn main() {
    let code = rust_cli::cli::run();
    std::process::exit(code);
}
