mod apply;
mod dedupe;
mod plan;
mod walk;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use plan::Plan;

#[derive(Parser)]
#[command(name = "mote", version, about = "Filesystem triage: route files, find duplicates")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Walk a tree and write a plan. Touches nothing.
    Scan {
        root: PathBuf,
        /// Where to write the plan; defaults to stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Execute a plan produced by `scan`.
    Apply {
        plan: PathBuf,
        /// Actually move files. Without this, the plan is only printed.
        #[arg(long)]
        commit: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Scan { root, out } => scan(root, out),
        Command::Apply { plan, commit } => apply(plan, commit),
    }
}

fn scan(root: PathBuf, out: Option<PathBuf>) -> Result<()> {
    let entries = walk::scan(&root);
    let duplicates = dedupe::find(&entries);

    let plan = Plan {
        root,
        scanned: entries.len(),
        // Routing is not wired up yet: the archive semantics are undecided.
        actions: Vec::new(),
        duplicates,
    };

    eprintln!(
        "scanned {} files, {} duplicate groups, {} reclaimable",
        plan.scanned,
        plan.duplicates.len(),
        human(plan.reclaimable()),
    );

    let json = plan.to_json()?;
    match out {
        Some(path) => std::fs::write(&path, json)
            .with_context(|| format!("writing {}", path.display()))?,
        None => println!("{json}"),
    }
    Ok(())
}

fn apply(path: PathBuf, commit: bool) -> Result<()> {
    let plan = Plan::from_json(
        &std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?,
    )?;

    if !commit {
        for action in &plan.actions {
            println!("{action:?}");
        }
        eprintln!("{} actions; re-run with --commit", plan.actions.len());
        return Ok(());
    }

    let done = apply::run(&plan)?;
    eprintln!("applied {done} actions");
    Ok(())
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
