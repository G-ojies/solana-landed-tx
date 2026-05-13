//! End-to-end demo: airdrops to a fresh keypair, then sends a self-transfer
//! through `send_and_confirm_with_retry` and prints landed telemetry.
//!
//! Usage:
//!     # against a local validator (default)
//!     solana-test-validator --reset &
//!     cargo run --example send_self_transfer
//!
//!     # against devnet
//!     cargo run --example send_self_transfer -- https://api.devnet.solana.com
//!
//! Localnet has no congestion data, so we use a fixed micro-lamport-per-CU price.
//! On a real network you'd swap to `FeeStrategy::Percentile(75)` to pull from
//! recent on-chain fee samples.

use std::sync::Arc;
use std::time::Duration;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_landed_tx::{FeeStrategy, RetryConfig, Sender};
use solana_sdk::{
    native_token::LAMPORTS_PER_SOL,
    signer::{keypair::Keypair, Signer},
};
use solana_system_interface::instruction as system_instruction;

const DEFAULT_RPC: &str = "http://127.0.0.1:8899";
const AIRDROP_LAMPORTS: u64 = LAMPORTS_PER_SOL;
const TRANSFER_LAMPORTS: u64 = 1_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc_url = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_RPC.into());
    println!("RPC: {rpc_url}");

    let rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url.clone(),
        CommitmentConfig::confirmed(),
    ));
    let version = rpc.get_version().await?;
    println!("RPC version: {} ({})", version.solana_core, version.feature_set.unwrap_or(0));

    let payer = Keypair::new();
    println!("payer: {}", payer.pubkey());

    println!("requesting airdrop of {AIRDROP_LAMPORTS} lamports...");
    let airdrop_sig = rpc.request_airdrop(&payer.pubkey(), AIRDROP_LAMPORTS).await?;
    wait_for_confirmation(&rpc, &airdrop_sig).await?;
    println!("airdrop confirmed: {airdrop_sig}");

    let starting_balance = rpc.get_balance(&payer.pubkey()).await?;
    println!("balance: {starting_balance} lamports");

    let sender = Sender::new(
        rpc.clone(),
        RetryConfig {
            max_attempts: 3,
            bump_factor: 2.0,
            per_attempt_timeout: Duration::from_secs(20),
            poll_interval: Duration::from_millis(500),
            cu_limit: 200_000,
            commitment: CommitmentConfig::confirmed(),
        },
    );

    let transfer = system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), TRANSFER_LAMPORTS);

    println!("sending self-transfer ({TRANSFER_LAMPORTS} lamports) via retry sender...");
    let result = sender
        .send_and_confirm_with_retry(
            &[transfer],
            &[&payer],
            &payer.pubkey(),
            &[payer.pubkey()],
            FeeStrategy::Fixed(1_000),
        )
        .await?;

    println!();
    println!("=== LANDED ===");
    println!("signature:      {}", result.signature);
    println!("attempts:       {}", result.attempts);
    println!("elapsed:        {} ms", result.elapsed_ms);
    println!("cu_price:       {} µLamports/CU", result.winning_cu_price_micro_lamports);
    println!("priority paid:  {} lamports", result.winning_priority_lamports);

    Ok(())
}

async fn wait_for_confirmation(
    rpc: &RpcClient,
    sig: &solana_sdk::signature::Signature,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if rpc.confirm_transaction(sig).await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Err(format!("timed out waiting for {sig} to confirm").into())
}
