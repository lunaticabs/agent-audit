use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const USAGE: &str = "\
usage:
  cargo xtask check
  cargo xtask fmt
  cargo xtask clippy
  cargo xtask test";

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let command = XtaskCommand::parse(env::args().skip(1))?;
    command.run()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XtaskCommand {
    Check,
    Fmt,
    Clippy,
    Test,
}

impl XtaskCommand {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let Some(raw) = args.next() else {
            return Err(USAGE.to_string());
        };
        let command = match raw.as_str() {
            "check" => Self::Check,
            "fmt" => Self::Fmt,
            "clippy" => Self::Clippy,
            "test" => Self::Test,
            _ => return Err(format!("unknown xtask command `{raw}`\n{USAGE}")),
        };
        if let Some(unexpected) = args.next() {
            return Err(format!("unexpected argument `{unexpected}`\n{USAGE}"));
        }
        Ok(command)
    }

    fn run(self) -> Result<(), String> {
        let workspace_root = workspace_root();
        let cargo = env::var("CARGO").unwrap_or_else(|_| String::from("cargo"));
        match self {
            Self::Check => run_check(&cargo, &workspace_root),
            Self::Fmt => run_single_step(&cargo, &workspace_root, CheckStep::FMT),
            Self::Clippy => run_single_step(&cargo, &workspace_root, CheckStep::CLIPPY),
            Self::Test => run_single_step(&cargo, &workspace_root, CheckStep::TEST),
        }
    }
}

fn run_check(cargo: &str, workspace_root: &Path) -> Result<(), String> {
    let steps = [CheckStep::FMT, CheckStep::CLIPPY, CheckStep::TEST];
    println!("running checks in {}", workspace_root.display());
    let mut outcomes = Vec::with_capacity(steps.len());
    let mut failed_steps = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        println!("==> [{}/{}] {}", index + 1, steps.len(), step.name);
        let outcome = run_step(cargo, workspace_root, *step)?;
        if !outcome.is_success() {
            failed_steps.push(step.name);
        }
        outcomes.push(outcome);
    }

    print_summary(&steps, &outcomes);
    if failed_steps.is_empty() {
        Ok(())
    } else {
        Err(format!("check failed: {}", failed_steps.join(", ")))
    }
}

fn run_single_step(cargo: &str, workspace_root: &Path, step: CheckStep) -> Result<(), String> {
    println!("running {} in {}", step.name, workspace_root.display());
    match run_step(cargo, workspace_root, step)? {
        StepOutcome::Passed => Ok(()),
        StepOutcome::Failed(detail) => Err(format!("{} failed ({detail})", step.name)),
    }
}

fn run_step(cargo: &str, workspace_root: &Path, step: CheckStep) -> Result<StepOutcome, String> {
    let status = Command::new(cargo)
        .args(step.args)
        .current_dir(workspace_root)
        .status()
        .map_err(|err| format!("failed to start `{}`: {err}", step.name))?;
    Ok(if status.success() {
        StepOutcome::Passed
    } else {
        StepOutcome::Failed(format_exit_status(status))
    })
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

#[derive(Clone, Copy)]
struct CheckStep {
    name: &'static str,
    args: &'static [&'static str],
}

impl CheckStep {
    const FMT: Self = Self::new("fmt", &["fmt", "--all", "--check"]);
    const CLIPPY: Self = Self::new(
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
    );
    const TEST: Self = Self::new("test", &["test", "--workspace", "--all-features"]);

    const fn new(name: &'static str, args: &'static [&'static str]) -> Self {
        Self { name, args }
    }
}

enum StepOutcome {
    Passed,
    Failed(String),
}

impl StepOutcome {
    const fn is_success(&self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[cfg(test)]
mod tests {
    use super::XtaskCommand;

    #[test]
    fn parse_recognizes_single_step_commands() {
        assert_eq!(
            XtaskCommand::parse(["fmt".to_string()].into_iter()).expect("parse fmt"),
            XtaskCommand::Fmt
        );
        assert_eq!(
            XtaskCommand::parse(["clippy".to_string()].into_iter()).expect("parse clippy"),
            XtaskCommand::Clippy
        );
        assert_eq!(
            XtaskCommand::parse(["test".to_string()].into_iter()).expect("parse test"),
            XtaskCommand::Test
        );
    }

    #[test]
    fn parse_rejects_unknown_command() {
        let error =
            XtaskCommand::parse(["unknown".to_string()].into_iter()).expect_err("unknown cmd");
        assert!(error.contains("unknown xtask command `unknown`"));
    }
}
