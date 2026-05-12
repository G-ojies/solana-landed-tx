use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Signature,
    signer::Signer,
    transaction::Transaction,
};

use crate::error::{LandedTxError, Result};
use crate::estimator::FeeEstimator;

/// How to pick the initial priority fee.
#[derive(Debug, Clone, Copy)]
pub enum FeeStrategy {
    /// Pay a fixed price (micro-lamports per CU). Use when you've already
    /// computed an estimate or you want a deterministic test.
    Fixed(u64),
    /// Pull the latest distribution from the estimator and pick this percentile.
    /// Valid range: 1..=100.
    Percentile(u8),
}

impl FeeStrategy {
    /// Default initial strategy: p75. Sensible mid-aggressive starting point.
    pub fn auto() -> Self {
        FeeStrategy::Percentile(75)
    }
}

#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Max attempts including the first send.
    pub max_attempts: u32,
    /// Multiplier applied to the fee on each retry. Must be > 1.0.
    pub bump_factor: f64,
    /// How long to wait for each attempt to confirm before giving up on it.
    pub per_attempt_timeout: Duration,
    /// How often to poll signature status during each attempt.
    pub poll_interval: Duration,
    /// Compute unit limit to set on every attempt. 200k is the network default
    /// when none is set; setting it explicitly is cheaper for simple txs and
    /// often required for complex ones.
    pub cu_limit: u32,
    /// Commitment level at which a tx is considered landed.
    pub commitment: CommitmentConfig,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            bump_factor: 1.5,
            per_attempt_timeout: Duration::from_secs(30),
            poll_interval: Duration::from_millis(1500),
            cu_limit: 200_000,
            commitment: CommitmentConfig::confirmed(),
        }
    }
}

/// Outcome of a successful send. Only the winning attempt's priority fee was
/// actually charged on-chain — dropped attempts cost nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandedTx {
    pub signature: Signature,
    pub attempts: u32,
    pub winning_cu_price_micro_lamports: u64,
    pub winning_priority_lamports: u64,
    pub elapsed_ms: u128,
}

pub struct Sender {
    rpc: Arc<RpcClient>,
    estimator: FeeEstimator,
    config: RetryConfig,
}

impl Sender {
    pub fn new(rpc: Arc<RpcClient>, config: RetryConfig) -> Self {
        let estimator = FeeEstimator::new(rpc.clone());
        Self { rpc, estimator, config }
    }

    /// Send a transaction and retry with bumped priority fees until it lands or
    /// `max_attempts` is exhausted.
    ///
    /// `writable_hints` should contain accounts the transaction writes to — they're
    /// passed to the fee estimator to scope the estimate to relevant slots.
    pub async fn send_and_confirm_with_retry(
        &self,
        user_instructions: &[Instruction],
        signers: &[&dyn Signer],
        payer: &Pubkey,
        writable_hints: &[Pubkey],
        strategy: FeeStrategy,
    ) -> Result<LandedTx> {
        let started = Instant::now();
        let mut cu_price = self.pick_initial_fee(writable_hints, strategy).await?;
        let mut sent_signatures: Vec<Signature> = Vec::with_capacity(self.config.max_attempts as usize);

        for attempt in 1..=self.config.max_attempts {
            let blockhash = self.rpc.get_latest_blockhash().await?;

            let mut instructions = priority_instructions(self.config.cu_limit, cu_price);
            instructions.extend_from_slice(user_instructions);

            let tx = Transaction::new_signed_with_payer(
                &instructions,
                Some(payer),
                signers,
                blockhash,
            );
            let sig = *tx.signatures.first().ok_or_else(|| {
                LandedTxError::Signer("transaction produced no signatures".into())
            })?;
            sent_signatures.push(sig);

            self.rpc.send_transaction(&tx).await?;

            if self.poll_until_landed(sig).await? {
                return Ok(LandedTx {
                    signature: sig,
                    attempts: attempt,
                    winning_cu_price_micro_lamports: cu_price,
                    winning_priority_lamports: priority_lamports(cu_price, self.config.cu_limit),
                    elapsed_ms: started.elapsed().as_millis(),
                });
            }

            cu_price = bump_fee(cu_price, self.config.bump_factor);
        }

        // Final sweep: any previously-sent attempt may have landed during the last poll window.
        if let Some(sig) = self.find_landed(&sent_signatures).await? {
            return Ok(LandedTx {
                signature: sig,
                attempts: self.config.max_attempts,
                winning_cu_price_micro_lamports: cu_price,
                winning_priority_lamports: priority_lamports(cu_price, self.config.cu_limit),
                elapsed_ms: started.elapsed().as_millis(),
            });
        }

        Err(LandedTxError::NotLanded { attempts: self.config.max_attempts })
    }

    async fn pick_initial_fee(&self, writable_hints: &[Pubkey], strategy: FeeStrategy) -> Result<u64> {
        match strategy {
            FeeStrategy::Fixed(f) => Ok(f),
            FeeStrategy::Percentile(p) => {
                let est = self.estimator.estimate(writable_hints).await?;
                Ok(match p {
                    0..=50 => est.p50,
                    51..=75 => est.p75,
                    76..=90 => est.p90,
                    _ => est.p95,
                })
            }
        }
    }

    async fn poll_until_landed(&self, sig: Signature) -> Result<bool> {
        let deadline = Instant::now() + self.config.per_attempt_timeout;
        while Instant::now() < deadline {
            tokio::time::sleep(self.config.poll_interval).await;
            if self.is_landed(sig).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn is_landed(&self, sig: Signature) -> Result<bool> {
        let statuses = self.rpc.get_signature_statuses(&[sig]).await?;
        Ok(statuses
            .value
            .into_iter()
            .next()
            .flatten()
            .map(|s| s.satisfies_commitment(self.config.commitment) && s.err.is_none())
            .unwrap_or(false))
    }

    async fn find_landed(&self, sigs: &[Signature]) -> Result<Option<Signature>> {
        if sigs.is_empty() {
            return Ok(None);
        }
        let statuses = self.rpc.get_signature_statuses(sigs).await?;
        for (sig, status) in sigs.iter().zip(statuses.value.into_iter()) {
            if let Some(s) = status {
                if s.satisfies_commitment(self.config.commitment) && s.err.is_none() {
                    return Ok(Some(*sig));
                }
            }
        }
        Ok(None)
    }
}

/// Build the ComputeBudget instructions to prepend: CU limit + CU price.
pub(crate) fn priority_instructions(cu_limit: u32, cu_price_micro_lamports: u64) -> Vec<Instruction> {
    vec![
        ComputeBudgetInstruction::set_compute_unit_limit(cu_limit),
        ComputeBudgetInstruction::set_compute_unit_price(cu_price_micro_lamports),
    ]
}

/// Bump a fee by `factor`, clamping below to ensure forward progress when the
/// previous fee was zero (zero * any factor is still zero — caller would loop forever).
pub(crate) fn bump_fee(current: u64, factor: f64) -> u64 {
    debug_assert!(factor > 1.0, "bump_factor must be > 1.0");
    let bumped = (current as f64 * factor) as u64;
    bumped.max(current.saturating_add(1))
}

/// Total priority fee in lamports for a given CU price and limit.
pub(crate) fn priority_lamports(cu_price_micro_lamports: u64, cu_limit: u32) -> u64 {
    let product = cu_price_micro_lamports as u128 * cu_limit as u128;
    (product / 1_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_grows_fee() {
        assert_eq!(bump_fee(100, 1.5), 150);
        assert_eq!(bump_fee(1000, 2.0), 2000);
    }

    #[test]
    fn bump_from_zero_makes_progress() {
        // Zero * any factor is zero — we'd loop forever. Floor to current+1.
        assert_eq!(bump_fee(0, 1.5), 1);
        assert_eq!(bump_fee(0, 2.0), 1);
    }

    #[test]
    fn bump_from_one_makes_progress() {
        // 1 * 1.5 = 1.5 → truncates to 1. Floor to 2.
        assert_eq!(bump_fee(1, 1.5), 2);
    }

    #[test]
    fn priority_lamports_basic() {
        // 1000 µLamports/CU * 200,000 CU = 200_000_000 µLamports = 200 lamports.
        assert_eq!(priority_lamports(1000, 200_000), 200);
    }

    #[test]
    fn priority_lamports_realistic_mainnet() {
        // p95 from earlier mainnet run: 63594 µLamports/CU * 200k CU
        // = 12_718_800_000 µLamports = 12718 lamports = ~0.0000127 SOL.
        assert_eq!(priority_lamports(63594, 200_000), 12718);
    }

    #[test]
    fn priority_lamports_overflow_safe() {
        // u64::MAX * 1.4M would overflow u64. We use u128 internally.
        // This shouldn't panic.
        let _ = priority_lamports(u64::MAX, 1_400_000);
    }

    #[test]
    fn priority_instructions_emits_two() {
        let ix = priority_instructions(200_000, 1234);
        assert_eq!(ix.len(), 2);
    }
}
