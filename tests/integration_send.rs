//! Live integration test against a real Solana RPC.
//!
//! Marked `#[ignore]` so it never runs in default `cargo test`. To run it:
//!
//!     # 1. Start a local validator (in another terminal):
//!     solana-test-validator --reset
//!
//!     # 2. Run the test:
//!     cargo test --test integration_send -- --ignored
//!
//! Alternatively point at any RPC (devnet, custom localnet, etc.):
//!
//!     SOLANA_LANDED_TX_TEST_RPC=https://api.devnet.solana.com \
//!         cargo test --test integration_send -- --ignored
//!
//! The test airdrops 1 SOL to a fresh keypair, then runs a self-transfer through
//! the full retry sender. On localnet (no congestion) it should land on attempt 1.

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

#[tokio::test]
#[ignore = "requires a running RPC; see file header for setup"]
async fn self_transfer_lands_through_retry_sender() {
    let rpc_url = std::env::var("SOLANA_LANDED_TX_TEST_RPC")
        .unwrap_or_else(|_| DEFAULT_RPC.into());
    eprintln!("RPC: {rpc_url}");

    let rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url,
        CommitmentConfig::confirmed(),
    ));
    rpc.get_version()
        .await
        .expect("RPC unreachable — start solana-test-validator?");

    let payer = Keypair::new();
    eprintln!("payer: {}", payer.pubkey());

    let airdrop_sig = rpc
        .request_airdrop(&payer.pubkey(), AIRDROP_LAMPORTS)
        .await
        .expect("airdrop request failed");
    wait_for_confirmation(&rpc, &airdrop_sig).await;

    let starting_balance = rpc.get_balance(&payer.pubkey()).await.unwrap();
    assert_eq!(
        starting_balance, AIRDROP_LAMPORTS,
        "expected airdrop to land 1 SOL"
    );

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

    // Fixed strategy: localnet has no congestion data, so Percentile would error.
    // A nominal 1000 µLamports/CU is enough to land trivially.
    let result = sender
        .send_and_confirm_with_retry(
            &[transfer],
            &[&payer],
            &payer.pubkey(),
            &[payer.pubkey()],
            FeeStrategy::Fixed(1_000),
        )
        .await
        .expect("send_and_confirm_with_retry failed");

    eprintln!(
        "landed: sig={} attempts={} elapsed_ms={} priority_lamports={}",
        result.signature,
        result.attempts,
        result.elapsed_ms,
        result.winning_priority_lamports
    );

    assert_eq!(
        result.attempts, 1,
        "localnet should land first attempt; multiple attempts implies misconfiguration"
    );
    assert_eq!(result.winning_cu_price_micro_lamports, 1_000);
    assert!(result.elapsed_ms < 30_000);
}

async fn wait_for_confirmation(rpc: &RpcClient, sig: &solana_sdk::signature::Signature) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(true) = rpc.confirm_transaction(sig).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    panic!("timed out waiting for {sig} to confirm");
}
