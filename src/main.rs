mod apply;
mod archive;
mod dedupe;
mod ident;
mod plan;
mod route;
mod rules;
mod walk;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use plan::Plan;
use rules::Rules;

#[derive(Parser)]
#[command(
    name = "mote",
    version,
    about = "Filesystem triage: route files, find duplicates"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a starter rules.toml.
    Init {
        /// Where to write it.
        #[arg(default_value = "rules.toml")]
        out: PathBuf,
    },
    /// Walk a tree and write a plan. Touches nothing.
    Scan {
        root: PathBuf,
        /// Routing rules.
        #[arg(short, long, default_value = "rules.toml")]
        rules: PathBuf,
        /// Where to write the plan; defaults to stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Open archives and file their contents individually, instead of
        /// treating each archive as one item to route.
        #[arg(long)]
        extract: bool,
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
        Command::Init { out } => init(out),
        Command::Scan {
            root,
            rules,
            out,
            extract,
        } => scan(root, rules, out, extract),
        Command::Apply { plan, commit } => apply(plan, commit),
    }
}

fn init(out: PathBuf) -> Result<()> {
    if out.exists() {
        bail!("{} already exists", out.display());
    }
    std::fs::write(&out, rules::TEMPLATE).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

fn scan(root: PathBuf, rules_path: PathBuf, out: Option<PathBuf>, extract: bool) -> Result<()> {
    let rules = Rules::load(&rules_path)?;

    let entries = walk::scan(&root);
    let duplicates = dedupe::find(&entries);
    let routed = route::build(&root, &entries, &duplicates, &rules, extract);

    let plan = Plan {
        root,
        scanned: entries.len(),
        actions: routed.actions,
        duplicates,
        unroutable: routed.unroutable,
    };

    let (moves, quarantines, extracts) = plan.tally();
    eprintln!(
        "scanned {} files: {moves} to move, {quarantines} to quarantine, \
         {extracts} archives to expand",
        plan.scanned,
    );
    eprintln!(
        "{} duplicate groups, {} reclaimable",
        plan.duplicates.len(),
        human(plan.reclaimable()),
    );
    if extracts > 0 {
        eprintln!(
            "{} to be written out of archives",
            human(plan.extracted_bytes())
        );
    }
    if !plan.unroutable.is_empty() {
        eprintln!(
            "{} files matched no rule and will be left alone",
            plan.unroutable.len()
        );
    }

    let json = plan.to_json()?;
    match out {
        Some(path) => {
            std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?
        }
        None => println!("{json}"),
    }
    Ok(())
}

fn apply(path: PathBuf, commit: bool) -> Result<()> {
    let plan = Plan::from_json(
        &std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
    )?;

    if !commit {
        for action in &plan.actions {
            match action {
                plan::Action::Move { from, to } => {
                    println!("move      {} -> {}", from.display(), to.display());
                }
                plan::Action::Quarantine { from, to, reason } => {
                    println!(
                        "quarantine {} -> {} ({reason})",
                        from.display(),
                        to.display()
                    );
                }
                plan::Action::Extract { from, members, .. } => {
                    println!("extract   {} ({} members)", from.display(), members.len());
                    for member in members {
                        println!(
                            "             {} -> {}",
                            member.name.display(),
                            member.to.display()
                        );
                    }
                }
            }
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
