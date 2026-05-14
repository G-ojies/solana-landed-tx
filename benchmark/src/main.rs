//! Reproducible Solana transaction-landing benchmark.
//!
//! Compares 6 fee strategies (no-fee, fixed-low, fixed-high, p75, p95, library-retry)
//! by sending self-transfers and recording landing rate, time-to-land, and priority
//! fees paid. Output: per-strategy CSVs + a summary CSV.
//!
//! Usage:
//!     # Localnet smoke test (no SOL needed)
//!     solana-test-validator --reset &
//!     cargo run -p solana-landed-tx-benchmark --release -- \
//!         --rpc http://127.0.0.1:8899 \
//!         --txs-per-strategy 5 \
//!         --output bench-results
//!
//!     # Real mainnet benchmark (requires funded keypair)
//!     cargo run -p solana-landed-tx-benchmark --release -- \
//!         --rpc https://api.mainnet-beta.solana.com \
//!         --keypair ~/.config/solana/id.json \
//!         --txs-per-strategy 50 \
//!         --output bench-mainnet
//!
//! Localnet has no congestion, so all strategies land with rate 100% and fees of 0 —
//! the framework is verified but the comparison is meaningful only on mainnet.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_landed_tx::{FeeStrategy, RetryConfig, Sender};
use solana_sdk::{
    native_token::LAMPORTS_PER_SOL,
    pubkey::Pubkey,
    signature::Signature,
    signer::{keypair::{read_keypair_file, Keypair}, Signer},
    transaction::Transaction,
};
use solana_system_interface::instruction as system_instruction;

const TRANSFER_LAMPORTS: u64 = 1;
const CU_LIMIT: u32 = 200_000;
const POLL_INTERVAL: Duration = Duration::from_millis(750);
const PER_TX_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Solana RPC URL.
    #[arg(long)]
    rpc: String,

    /// Path to a funded keypair JSON. If omitted, a fresh keypair is generated
    /// and airdropped (only works on localnet/devnet).
    #[arg(long)]
    keypair: Option<PathBuf>,

    /// Number of transactions to send per strategy.
    #[arg(long, default_value_t = 10)]
    txs_per_strategy: usize,

    /// Output directory for CSVs. Created if missing.
    #[arg(long)]
    output: PathBuf,

    /// Sleep between successive tx sends (within a strategy). Helps avoid
    /// blockhash collisions and gives leaders time to include prior txs.
    #[arg(long, default_value_t = 1000)]
    inter_tx_ms: u64,

    /// USDC mint, used as a writable hint for the fee estimator on mainnet.
    /// Override to scope estimates to a different hot account.
    #[arg(long, default_value = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")]
    writable_hint: String,
}

#[derive(Debug, Clone, Copy)]
enum Strategy {
    /// No ComputeBudgetInstruction at all — what a naive sender does.
    NoFee,
    /// Single-shot send with a fixed low CU price.
    FixedLow,
    /// Single-shot send with a fixed high CU price.
    FixedHigh,
    /// Single-shot send at the library's p75 estimate.
    P75,
    /// Single-shot send at the library's p95 estimate.
    P95,
    /// Full library use: send_and_confirm_with_retry, starting at p75, bumping 1.5x.
    LibraryRetry,
}

impl Strategy {
    fn name(self) -> &'static str {
        match self {
            Strategy::NoFee => "no_fee",
            Strategy::FixedLow => "fixed_low",
            Strategy::FixedHigh => "fixed_high",
            Strategy::P75 => "p75",
            Strategy::P95 => "p95",
            Strategy::LibraryRetry => "library_retry",
        }
    }

    fn all() -> [Strategy; 6] {
        [
            Strategy::NoFee,
            Strategy::FixedLow,
            Strategy::FixedHigh,
            Strategy::P75,
            Strategy::P95,
            Strategy::LibraryRetry,
        ]
    }
}

#[derive(Debug, Clone, Serialize)]
struct Sample {
    strategy: String,
    tx_index: usize,
    signature: Option<String>,
    sent_ms_from_start: u128,
    landed_ms_from_start: Option<u128>,
    time_to_land_ms: Option<u128>,
    cu_price_micro_lamports: u64,
    priority_lamports_paid: u64,
    attempts: u32,
    landed: bool,
}

#[derive(Debug, Serialize)]
struct StrategySummary {
    strategy: String,
    txs: usize,
    landed: usize,
    landing_rate_pct: f64,
    mean_time_to_land_ms: Option<f64>,
    median_time_to_land_ms: Option<u128>,
    mean_priority_lamports: Option<f64>,
    total_priority_lamports: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.output)
        .with_context(|| format!("creating output dir {}", args.output.display()))?;

    let rpc = Arc::new(RpcClient::new_with_commitment(
        args.rpc.clone(),
        CommitmentConfig::confirmed(),
    ));
    let version = rpc.get_version().await.context("getVersion")?;
    eprintln!("RPC ok: solana-core {}", version.solana_core);

    let payer = load_or_airdrop(&rpc, args.keypair.as_deref()).await?;
    eprintln!("payer: {}", payer.pubkey());
    let starting_balance = rpc.get_balance(&payer.pubkey()).await?;
    eprintln!("balance: {} lamports", starting_balance);

    let writable_hint: Pubkey = args
        .writable_hint
        .parse()
        .context("invalid --writable-hint pubkey")?;

    let bench_start = Instant::now();
    let mut all_samples: Vec<Sample> = Vec::new();
    let mut summaries: Vec<StrategySummary> = Vec::new();

    for strategy in Strategy::all() {
        eprintln!("\n=== {} ===", strategy.name());
        let mut samples = Vec::with_capacity(args.txs_per_strategy);
        for i in 0..args.txs_per_strategy {
            let sample = run_one(
                strategy,
                i,
                &rpc,
                &payer,
                &writable_hint,
                bench_start,
            )
            .await;
            eprintln!(
                "  [{}/{}] {} {}",
                i + 1,
                args.txs_per_strategy,
                if sample.landed { "✓" } else { "✗" },
                sample
                    .time_to_land_ms
                    .map(|t| format!("{t}ms"))
                    .unwrap_or_else(|| "—".into()),
            );
            samples.push(sample);
            tokio::time::sleep(Duration::from_millis(args.inter_tx_ms)).await;
        }

        // Per-strategy CSV
        let path = args.output.join(format!("{}.csv", strategy.name()));
        let mut w = csv::Writer::from_path(&path).with_context(|| format!("opening {path:?}"))?;
        for s in &samples {
            w.serialize(s)?;
        }
        w.flush()?;

        let summary = summarize(strategy.name(), &samples);
        eprintln!(
            "  landed: {}/{}  rate: {:.1}%  total_priority: {} lamports",
            summary.landed,
            summary.txs,
            summary.landing_rate_pct,
            summary.total_priority_lamports,
        );
        summaries.push(summary);
        all_samples.extend(samples);
    }

    // Summary CSV
    let summary_path = args.output.join("summary.csv");
    let mut w = csv::Writer::from_path(&summary_path)?;
    for s in &summaries {
        w.serialize(s)?;
    }
    w.flush()?;

    // Combined raw CSV for easier post-hoc analysis.
    let raw_path = args.output.join("raw.csv");
    let mut w = csv::Writer::from_path(&raw_path)?;
    for s in &all_samples {
        w.serialize(s)?;
    }
    w.flush()?;

    eprintln!("\nresults written to {}", args.output.display());
    Ok(())
}

async fn load_or_airdrop(rpc: &RpcClient, keypair_path: Option<&std::path::Path>) -> Result<Keypair> {
    if let Some(path) = keypair_path {
        return read_keypair_file(path)
            .map_err(|e| anyhow::anyhow!("reading keypair {path:?}: {e}"));
    }
    let kp = Keypair::new();
    let sig = rpc
        .request_airdrop(&kp.pubkey(), LAMPORTS_PER_SOL)
        .await
        .context("airdrop (only works on localnet/devnet)")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if rpc.confirm_transaction(&sig).await.unwrap_or(false) {
            return Ok(kp);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    anyhow::bail!("airdrop did not confirm");
}

async fn run_one(
    strategy: Strategy,
    tx_index: usize,
    rpc: &Arc<RpcClient>,
    payer: &Keypair,
    writable_hint: &Pubkey,
    bench_start: Instant,
) -> Sample {
    let sent_at = Instant::now();
    let sent_ms_from_start = sent_at.duration_since(bench_start).as_millis();

    match strategy {
        Strategy::NoFee => send_raw_no_priority(rpc, payer, bench_start, tx_index, sent_ms_from_start).await,
        Strategy::FixedLow => send_raw_with_price(rpc, payer, 1_000, bench_start, tx_index, sent_ms_from_start, strategy.name()).await,
        Strategy::FixedHigh => send_raw_with_price(rpc, payer, 50_000, bench_start, tx_index, sent_ms_from_start, strategy.name()).await,
        Strategy::P75 => send_via_library(rpc, payer, writable_hint, FeeStrategy::Percentile(75), false, bench_start, tx_index, sent_ms_from_start, strategy.name()).await,
        Strategy::P95 => send_via_library(rpc, payer, writable_hint, FeeStrategy::Percentile(95), false, bench_start, tx_index, sent_ms_from_start, strategy.name()).await,
        Strategy::LibraryRetry => send_via_library(rpc, payer, writable_hint, FeeStrategy::Percentile(75), true, bench_start, tx_index, sent_ms_from_start, strategy.name()).await,
    }
}

async fn send_raw_no_priority(
    rpc: &Arc<RpcClient>,
    payer: &Keypair,
    bench_start: Instant,
    tx_index: usize,
    sent_ms_from_start: u128,
) -> Sample {
    let blockhash = match rpc.get_latest_blockhash().await {
        Ok(bh) => bh,
        Err(e) => return failed_sample("no_fee", tx_index, sent_ms_from_start, 0, format!("blockhash: {e}")),
    };
    let ix = system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), TRANSFER_LAMPORTS);
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);
    let sig = match rpc.send_transaction(&tx).await {
        Ok(s) => s,
        Err(e) => return failed_sample("no_fee", tx_index, sent_ms_from_start, 0, format!("send: {e}")),
    };
    poll_for_landing(rpc, sig, "no_fee", tx_index, sent_ms_from_start, 0, 0, 1, bench_start).await
}

#[allow(clippy::too_many_arguments)]
async fn send_raw_with_price(
    rpc: &Arc<RpcClient>,
    payer: &Keypair,
    cu_price: u64,
    bench_start: Instant,
    tx_index: usize,
    sent_ms_from_start: u128,
    name: &str,
) -> Sample {
    let blockhash = match rpc.get_latest_blockhash().await {
        Ok(bh) => bh,
        Err(e) => return failed_sample(name, tx_index, sent_ms_from_start, cu_price, format!("blockhash: {e}")),
    };
    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(CU_LIMIT),
        ComputeBudgetInstruction::set_compute_unit_price(cu_price),
        system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), TRANSFER_LAMPORTS),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer], blockhash);
    let sig = match rpc.send_transaction(&tx).await {
        Ok(s) => s,
        Err(e) => return failed_sample(name, tx_index, sent_ms_from_start, cu_price, format!("send: {e}")),
    };
    let priority_lamports = (cu_price as u128 * CU_LIMIT as u128 / 1_000_000) as u64;
    poll_for_landing(rpc, sig, name, tx_index, sent_ms_from_start, cu_price, priority_lamports, 1, bench_start).await
}

#[allow(clippy::too_many_arguments)]
async fn send_via_library(
    rpc: &Arc<RpcClient>,
    payer: &Keypair,
    writable_hint: &Pubkey,
    strategy: FeeStrategy,
    use_retry: bool,
    bench_start: Instant,
    tx_index: usize,
    sent_ms_from_start: u128,
    name: &str,
) -> Sample {
    let config = RetryConfig {
        max_attempts: if use_retry { 5 } else { 1 },
        bump_factor: 1.5,
        per_attempt_timeout: if use_retry { Duration::from_secs(15) } else { PER_TX_TIMEOUT },
        poll_interval: POLL_INTERVAL,
        cu_limit: CU_LIMIT,
        commitment: CommitmentConfig::confirmed(),
    };
    let sender = Sender::new(rpc.clone(), config);
    let transfer = system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), TRANSFER_LAMPORTS);
    match sender
        .send_and_confirm_with_retry(
            &[transfer],
            &[payer],
            &payer.pubkey(),
            &[*writable_hint],
            strategy,
        )
        .await
    {
        Ok(landed) => {
            let landed_ms = bench_start.elapsed().as_millis();
            Sample {
                strategy: name.to_string(),
                tx_index,
                signature: Some(landed.signature.to_string()),
                sent_ms_from_start,
                landed_ms_from_start: Some(landed_ms),
                time_to_land_ms: Some(landed_ms - sent_ms_from_start),
                cu_price_micro_lamports: landed.winning_cu_price_micro_lamports,
                priority_lamports_paid: landed.winning_priority_lamports,
                attempts: landed.attempts,
                landed: true,
            }
        }
        Err(e) => failed_sample(name, tx_index, sent_ms_from_start, 0, format!("send: {e}")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn poll_for_landing(
    rpc: &Arc<RpcClient>,
    sig: Signature,
    name: &str,
    tx_index: usize,
    sent_ms_from_start: u128,
    cu_price: u64,
    priority_lamports: u64,
    attempts: u32,
    bench_start: Instant,
) -> Sample {
    let deadline = Instant::now() + PER_TX_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(true) = rpc.confirm_transaction(&sig).await {
            let landed_ms = bench_start.elapsed().as_millis();
            return Sample {
                strategy: name.to_string(),
                tx_index,
                signature: Some(sig.to_string()),
                sent_ms_from_start,
                landed_ms_from_start: Some(landed_ms),
                time_to_land_ms: Some(landed_ms - sent_ms_from_start),
                cu_price_micro_lamports: cu_price,
                priority_lamports_paid: priority_lamports,
                attempts,
                landed: true,
            };
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Sample {
        strategy: name.to_string(),
        tx_index,
        signature: Some(sig.to_string()),
        sent_ms_from_start,
        landed_ms_from_start: None,
        time_to_land_ms: None,
        cu_price_micro_lamports: cu_price,
        priority_lamports_paid: 0,
        attempts,
        landed: false,
    }
}

fn failed_sample(name: &str, tx_index: usize, sent_ms_from_start: u128, cu_price: u64, reason: String) -> Sample {
    eprintln!("    error: {reason}");
    Sample {
        strategy: name.to_string(),
        tx_index,
        signature: None,
        sent_ms_from_start,
        landed_ms_from_start: None,
        time_to_land_ms: None,
        cu_price_micro_lamports: cu_price,
        priority_lamports_paid: 0,
        attempts: 0,
        landed: false,
    }
}

fn summarize(name: &str, samples: &[Sample]) -> StrategySummary {
    let txs = samples.len();
    let landed_samples: Vec<&Sample> = samples.iter().filter(|s| s.landed).collect();
    let landed = landed_samples.len();
    let landing_rate_pct = if txs == 0 {
        0.0
    } else {
        100.0 * landed as f64 / txs as f64
    };

    let times: Vec<u128> = landed_samples.iter().filter_map(|s| s.time_to_land_ms).collect();
    let mean_time = if times.is_empty() {
        None
    } else {
        Some(times.iter().sum::<u128>() as f64 / times.len() as f64)
    };
    let median_time = if times.is_empty() {
        None
    } else {
        let mut sorted = times.clone();
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    };

    let priority_paid: Vec<u64> = landed_samples.iter().map(|s| s.priority_lamports_paid).collect();
    let mean_priority = if priority_paid.is_empty() {
        None
    } else {
        Some(priority_paid.iter().sum::<u64>() as f64 / priority_paid.len() as f64)
    };
    let total_priority: u64 = priority_paid.iter().sum();

    StrategySummary {
        strategy: name.to_string(),
        txs,
        landed,
        landing_rate_pct,
        mean_time_to_land_ms: mean_time,
        median_time_to_land_ms: median_time,
        mean_priority_lamports: mean_priority,
        total_priority_lamports: total_priority,
    }
}
