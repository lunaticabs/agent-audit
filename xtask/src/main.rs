use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const USAGE: &str = "usage: cargo xtask check";

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("check") => {
            if let Some(unexpected) = args.next() {
                return Err(format!("unexpected argument `{unexpected}`\n{USAGE}"));
            }
            run_check()
        }
        Some(command) => Err(format!("unknown xtask command `{command}`\n{USAGE}")),
        None => Err(USAGE.to_string()),
    }
}

fn run_check() -> Result<(), String> {
    let workspace_root = workspace_root();
    let cargo = env::var("CARGO").unwrap_or_else(|_| String::from("cargo"));
    let steps = [
        CheckStep::new("fmt", &["fmt", "--all", "--check"]),
        CheckStep::new(
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        ),
        CheckStep::new("test", &["test", "--workspace", "--all-features"]),
    ];

    println!("running checks in {}", workspace_root.display());
    let mut outcomes = Vec::with_capacity(steps.len());
    let mut failed_steps = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        println!("==> [{}/{}] {}", index + 1, steps.len(), step.name);
        let status = Command::new(&cargo)
            .args(step.args)
            .current_dir(&workspace_root)
            .status()
            .map_err(|err| format!("failed to start `{}`: {err}", step.name))?;
        if !status.success() {
            let detail = format_exit_status(status);
            failed_steps.push(step.name);
            outcomes.push(StepOutcome::Failed(detail));
            continue;
        }
        outcomes.push(StepOutcome::Passed);
    }

    print_summary(&steps, &outcomes);
    if failed_steps.is_empty() {
        Ok(())
    } else {
        Err(format!("check failed: {}", failed_steps.join(", ")))
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest should have a workspace root parent")
        .to_path_buf()
}

fn print_summary(steps: &[CheckStep], outcomes: &[StepOutcome]) {
    println!("check summary:");
    for (step, outcome) in steps.iter().zip(outcomes) {
        match outcome {
            StepOutcome::Passed => println!("  {}: ok", step.name),
            StepOutcome::Failed(detail) => println!("  {}: failed ({detail})", step.name),
        }
    }
}

fn format_exit_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => String::from("signal termination"),
    }
}

struct CheckStep {
    name: &'static str,
    args: &'static [&'static str],
}

impl CheckStep {
    const fn new(name: &'static str, args: &'static [&'static str]) -> Self {
        Self { name, args }
    }
}

enum StepOutcome {
    Passed,
    Failed(String),
}
