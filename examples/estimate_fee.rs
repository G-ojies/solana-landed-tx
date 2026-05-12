//! Print the current priority fee distribution for a given RPC.
//!
//! Usage:
//!     cargo run --example estimate_fee -- <RPC_URL>
//!     cargo run --example estimate_fee -- https://api.mainnet-beta.solana.com

use std::str::FromStr;
use std::sync::Arc;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_landed_tx::FeeEstimator;
use solana_sdk::pubkey::Pubkey;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://api.mainnet-beta.solana.com".to_string());

    // Optional 2nd arg: a writable account to scope the fee query to slots
    // that touched it. Defaults to the USDC mint, which is touched constantly.
    let account = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string());
    let writable = Pubkey::from_str(&account)?;

    println!("RPC:     {rpc_url}");
    println!("Scoped to writable account: {writable}");

    let rpc = Arc::new(RpcClient::new(rpc_url));
    let estimator = FeeEstimator::new(rpc);

    let est = estimator.estimate(&[writable]).await?;

    println!("samples: {}", est.samples);
    println!("p50:     {} µLamports/CU", est.p50);
    println!("p75:     {} µLamports/CU", est.p75);
    println!("p90:     {} µLamports/CU", est.p90);
    println!("p95:     {} µLamports/CU", est.p95);
    println!("max:     {} µLamports/CU", est.max);
    println!("mean:    {} µLamports/CU", est.mean);

    let cu_limit: u64 = 200_000;
    let lamports = est.p75.saturating_mul(cu_limit) / 1_000_000;
    println!("\nAt p75 with a {cu_limit} CU budget, you'd pay {lamports} lamports in priority fees.");

    Ok(())
}
