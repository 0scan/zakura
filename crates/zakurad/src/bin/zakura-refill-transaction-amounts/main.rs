//! Standalone one-time explorer transaction metadata refill utility.

#![allow(clippy::print_stderr, clippy::print_stdout)]

use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre::Result;
use zakura_chain::parameters::Network;
use zakura_state::{RefillTransactionAmountsOptions, RefillTransactionAmountsSummary};

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

    /// Decoded historical source transactions retained between batches.
    /// Defaults to 10,000 entries; zero disables the cache.
    #[clap(long)]
    source_cache_entries: Option<usize>,

    /// Stop after this many legacy records, for benchmarking or a partial resumable run.
    #[clap(long)]
    limit: Option<u64>,

    /// Write changes. Without this flag, the command performs a read-only dry run.
    #[clap(long)]
    confirm: bool,
}

fn main() {
    if let Err(error) = color_eyre::install() {
        eprintln!("failed to install error handler: {error}");
        std::process::exit(1);
    }

    if let Err(error) = run(Args::parse()) {
        eprintln!("failed to refill explorer transaction metadata: {error:?}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let config = zakura_state::Config {
        cache_dir: args.cache_dir,
        ..Default::default()
    };
    let defaults = RefillTransactionAmountsOptions::default();
    let options = RefillTransactionAmountsOptions {
        batch_size: args.batch_size,
        workers: args.workers.unwrap_or(defaults.workers),
        source_cache_entries: args
            .source_cache_entries
            .unwrap_or(defaults.source_cache_entries),
        limit: args.limit,
        dry_run: !args.confirm,
    };

    if args.confirm {
        println!("opening the primary database; zakurad must be stopped");
    } else {
        println!("dry run: no database changes will be written");
    }
    println!(
        "workers: {}, source cache entries: {}",
        options.workers, options.source_cache_entries
    );

    let summary = zakura_state::refill_transaction_amounts(config, &args.network, options)?;
    print_summary(&summary);

    if !args.confirm {
        println!("no changes written: stop zakurad and pass --confirm to apply the refill");
    } else if !summary.scan_complete {
        println!("partial refill complete: run the command again without --limit to finish");
    }

    Ok(())
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
