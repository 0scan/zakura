//! Standalone one-time explorer transaction metadata refill utility.

#![allow(clippy::print_stderr, clippy::print_stdout)]

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use color_eyre::eyre::{bail, Result};
use tracing_subscriber::EnvFilter;
use zakura_chain::parameters::Network;
use zakura_state::{
    AppliedPreparedTransactionAmountRefillSummary, PreparedTransactionAmountRefillSummary,
    RefillTransactionAmountsOptions, RefillTransactionAmountsSummary,
};

/// Refill explorer transaction amounts and primary endpoints in an archive database.
#[derive(Debug, Parser)]
struct Args {
    /// Path to the Zakura cache directory containing the state database.
    #[clap(long, short)]
    cache_dir: PathBuf,

    /// Network of the state database.
    #[clap(long, short, required = true)]
    network: Network,

    /// Number of transaction metadata updates per atomic RocksDB batch.
    #[clap(long, default_value_t = 10_000)]
    batch_size: usize,

    /// Worker threads for historical transaction reads and endpoint derivation.
    /// Defaults to one fewer than the available CPU count.
    #[clap(long)]
    workers: Option<usize>,

    /// Decoded historical source outputs retained between batches.
    /// Defaults to 10,000 entries; zero disables the cache.
    #[clap(long)]
    source_cache_entries: Option<usize>,

    #[command(subcommand)]
    command: Command,
}

/// Refill workflow phase.
#[derive(Debug, Subcommand)]
enum Command {
    /// Calculate a complete snapshot artifact while zakurad remains online.
    Prepare {
        /// New artifact path. Existing files are never replaced.
        #[clap(long)]
        output: PathBuf,

        /// Partial artifact from an interrupted prepare run.
        #[clap(long)]
        resume_from: Option<PathBuf>,
    },

    /// Import a prepared artifact and calculate records appended after its snapshot.
    Apply {
        /// Artifact created by the prepare command.
        #[clap(long)]
        input: PathBuf,

        /// Confirm opening and modifying the primary RocksDB database.
        #[clap(long)]
        confirm: bool,
    },

    /// Run the original direct calculation path for benchmarking or fallback.
    Direct {
        /// Stop after this many legacy records for a benchmark or partial resumable run.
        #[clap(long)]
        limit: Option<u64>,

        /// Write changes. Without this flag, this command performs a read-only dry run.
        #[clap(long)]
        confirm: bool,
    },
}

fn main() {
    if let Err(error) = color_eyre::install() {
        eprintln!("failed to install error handler: {error}");
        std::process::exit(1);
    }

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("zakura_state=info"));
    if let Err(error) = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init()
    {
        eprintln!("failed to install tracing subscriber: {error}");
        std::process::exit(1);
    }

    if let Err(error) = run(Args::parse()) {
        eprintln!("failed to refill explorer transaction metadata: {error:?}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let config = zakura_state::Config {
        cache_dir: args.cache_dir.clone(),
        ..Default::default()
    };
    let defaults = RefillTransactionAmountsOptions::default();
    let base_options = RefillTransactionAmountsOptions {
        batch_size: args.batch_size,
        workers: args.workers.unwrap_or(defaults.workers),
        source_cache_entries: args
            .source_cache_entries
            .unwrap_or(defaults.source_cache_entries),
        limit: None,
        dry_run: true,
    };
    println!(
        "workers: {}, source cache entries: {}",
        base_options.workers, base_options.source_cache_entries
    );

    match args.command {
        Command::Prepare {
            output,
            resume_from,
        } => {
            println!("preparing from a read-only secondary; zakurad may remain online");
            let summary = if let Some(partial) = resume_from {
                println!("recovering checkpoint: {}", partial.display());
                zakura_state::resume_prepared_transaction_amount_refill(
                    config,
                    &args.network,
                    base_options,
                    &output,
                    &partial,
                )?
            } else {
                let partial =
                    zakura_state::prepared_transaction_amount_refill_partial_path(&output);
                println!("durable checkpoint: {}", partial.display());
                zakura_state::prepare_transaction_amount_refill(
                    config,
                    &args.network,
                    base_options,
                    &output,
                )?
            };
            print_prepared_summary(&summary, &output);
        }
        Command::Apply { input, confirm } => {
            if !confirm {
                bail!("apply modifies RocksDB; stop zakurad and pass --confirm");
            }
            println!("opening the primary database; zakurad must be stopped");
            let summary = zakura_state::apply_prepared_transaction_amount_refill(
                config,
                &args.network,
                base_options,
                &input,
            )?;
            print_applied_summary(&summary);
        }
        Command::Direct { limit, confirm } => {
            if confirm {
                println!("opening the primary database; zakurad must be stopped");
            } else {
                println!("dry run: no database changes will be written");
            }
            let options = RefillTransactionAmountsOptions {
                limit,
                dry_run: !confirm,
                ..base_options
            };
            let summary = zakura_state::refill_transaction_amounts(config, &args.network, options)?;
            print_summary(&summary);

            if !confirm {
                println!(
                    "no changes written: use prepare, or stop zakurad and rerun with --confirm"
                );
            } else if !summary.scan_complete {
                println!("partial refill complete: run direct again without --limit to finish");
            }
        }
    }

    Ok(())
}

fn print_prepared_summary(summary: &PreparedTransactionAmountRefillSummary, output: &Path) {
    println!("prepared transaction metadata refill:");
    println!("  source tip height: {}", summary.source_tip_height.0);
    println!("  source tip hash: {}", summary.source_tip_hash);
    println!("  prepared records: {}", summary.prepared_records);
    println!("  resumed records: {}", summary.resumed_records);
    println!("  output: {}", output.display());
    println!("  elapsed seconds: {}", summary.elapsed_seconds);
}

fn print_applied_summary(summary: &AppliedPreparedTransactionAmountRefillSummary) {
    println!("applied prepared transaction metadata refill:");
    println!("  imported records: {}", summary.imported_records);
    println!("  resumed records: {}", summary.resumed_records);
    println!(
        "  calculated tail records: {}",
        summary.tail_refill.refilled_records
    );
    println!("  reached index end: {}", summary.tail_refill.scan_complete);
    println!("  schema updated: {}", summary.tail_refill.schema_updated);
    println!("  elapsed seconds: {}", summary.elapsed_seconds);
}

fn print_summary(summary: &RefillTransactionAmountsSummary) {
    println!("transaction metadata refill:");
    println!("  scanned records: {}", summary.scanned_records);
    println!("  calculated legacy records: {}", summary.refilled_records);
    println!(
        "  already-refilled records: {}",
        summary.already_refilled_records
    );
    println!("  reached index end: {}", summary.scan_complete);
    println!("  schema updated: {}", summary.schema_updated);
    println!("  elapsed seconds: {}", summary.elapsed_seconds);
}
