#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use holdout_safe::{Config, Manifest, audit, plan, split};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print a deterministic fold manifest without writing datasets.
    Plan(PlanArgs),
    /// Write one JSONL file per fold plus manifest.json.
    Split(SplitArgs),
    /// Verify that an input still reproduces a saved manifest.
    Audit(AuditArgs),
}

#[derive(Debug, clap::Args)]
struct CommonArgs {
    /// Input JSON Lines file.
    #[arg(long)]
    input: PathBuf,
    /// Dotted path to the entity/group identifier.
    #[arg(long)]
    group_field: String,
    /// Optional dotted path to a label used for stratification.
    #[arg(long)]
    label_field: Option<String>,
    /// Number of folds to create (2-256).
    #[arg(long, default_value_t = 5)]
    folds: usize,
    /// Seed used for deterministic group ordering.
    #[arg(long, default_value = "holdout-safe")]
    seed: String,
}

impl CommonArgs {
    fn config(&self) -> Config {
        Config {
            group_field: self.group_field.clone(),
            label_field: self.label_field.clone(),
            folds: self.folds,
            seed: self.seed.clone(),
        }
    }
}

#[derive(Debug, clap::Args)]
struct PlanArgs {
    #[command(flatten)]
    common: CommonArgs,
    /// Pretty-print JSON output.
    #[arg(long)]
    pretty: bool,
}

#[derive(Debug, clap::Args)]
struct SplitArgs {
    #[command(flatten)]
    common: CommonArgs,
    /// New or empty directory for fold files and manifest.json.
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(Debug, clap::Args)]
struct AuditArgs {
    /// Original JSON Lines input.
    #[arg(long)]
    input: PathBuf,
    /// Manifest produced by plan or split.
    #[arg(long)]
    manifest: PathBuf,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    match cli.command {
        Command::Plan(args) => {
            let input = fs::read(&args.common.input)?;
            let manifest = plan(&input, args.common.config())?;
            print_json(&manifest, args.pretty)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Split(args) => {
            prepare_output_dir(&args.out_dir)?;
            let input = fs::read(&args.common.input)?;
            let (manifest, folds) = split(&input, args.common.config())?;
            for (index, contents) in folds.into_iter().enumerate() {
                fs::write(
                    args.out_dir.join(format!("fold-{index:03}.jsonl")),
                    contents,
                )?;
            }
            fs::write(
                args.out_dir.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest)?,
            )?;
            println!(
                "wrote {} records from {} groups into {} folds at {}",
                manifest.records,
                manifest.groups,
                manifest.config.folds,
                args.out_dir.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Audit(args) => {
            let input = fs::read(&args.input)?;
            let manifest: Manifest = serde_json::from_slice(&fs::read(&args.manifest)?)?;
            let report = audit(&input, &manifest)?;
            if args.json {
                print_json(&report, true)?;
            } else if report.valid {
                println!(
                    "VALID: {} records, {} groups, sha256:{}",
                    report.records, report.groups, report.input_sha256
                );
            } else {
                println!("INVALID:");
                for violation in &report.violations {
                    println!("- {violation}");
                }
            }
            Ok(if report.valid {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            })
        }
    }
}

fn prepare_output_dir(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        if !path.is_dir() {
            return Err(format!("output path is not a directory: {}", path.display()).into());
        }
        if fs::read_dir(path)?.next().is_some() {
            return Err(format!("output directory is not empty: {}", path.display()).into());
        }
    } else {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

fn print_json<T: Serialize>(value: &T, pretty: bool) -> Result<(), serde_json::Error> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}
