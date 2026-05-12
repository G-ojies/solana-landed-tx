use std::sync::Arc;

use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

use crate::error::{LandedTxError, Result};

/// Priority fee estimate, in micro-lamports per compute unit.
///
/// These are the raw per-CU prices to set via `ComputeBudgetInstruction::SetComputeUnitPrice`.
/// To compute the total priority fee in lamports for a transaction:
///     priority_lamports = (cu_price * cu_limit) / 1_000_000
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeEstimate {
    pub p50: u64,
    pub p75: u64,
    pub p90: u64,
    pub p95: u64,
    pub max: u64,
    pub mean: u64,
    /// Number of non-zero slot samples used in the estimate.
    pub samples: usize,
}

impl FeeEstimate {
    /// Reasonable default for a "land it" tx: p75 is usually enough during normal load,
    /// p90+ for congestion. Callers should pick deliberately; this is a hint only.
    pub fn recommended(&self) -> u64 {
        self.p75
    }
}

pub struct FeeEstimator {
    rpc: Arc<RpcClient>,
}

impl FeeEstimator {
    pub fn new(rpc: Arc<RpcClient>) -> Self {
        Self { rpc }
    }

    /// Query recent prioritization fees from the RPC and compute percentiles.
    ///
    /// Passing `writable_accounts` narrows the sample to slots that included a transaction
    /// writing to at least one of those accounts — a more relevant signal when you know
    /// which accounts your transaction will touch. Pass `&[]` for a global estimate.
    pub async fn estimate(&self, writable_accounts: &[Pubkey]) -> Result<FeeEstimate> {
        let samples = self
            .rpc
            .get_recent_prioritization_fees(writable_accounts)
            .await?;

        let fees: Vec<u64> = samples
            .into_iter()
            .map(|s| s.prioritization_fee)
            .collect();

        if fees.is_empty() {
            return Err(LandedTxError::NoFeeSamples);
        }

        Ok(compute_percentiles(&fees))
    }
}

/// Compute percentile-based fee estimates from a non-empty slice of raw fee samples.
///
/// Uses the nearest-rank method on a sorted copy. Returns mean, max, and the four
/// canonical percentiles (50/75/90/95).
pub(crate) fn compute_percentiles(fees: &[u64]) -> FeeEstimate {
    debug_assert!(!fees.is_empty(), "compute_percentiles requires non-empty input");

    let mut sorted = fees.to_vec();
    sorted.sort_unstable();

    let n = sorted.len();
    let sum: u128 = sorted.iter().map(|f| *f as u128).sum();
    let mean = (sum / n as u128) as u64;

    FeeEstimate {
        p50: percentile(&sorted, 50),
        p75: percentile(&sorted, 75),
        p90: percentile(&sorted, 90),
        p95: percentile(&sorted, 95),
        max: *sorted.last().unwrap(),
        mean,
        samples: n,
    }
}

/// Nearest-rank percentile on an already-sorted slice. `p` is 0..=100.
fn percentile(sorted: &[u64], p: u8) -> u64 {
    let n = sorted.len();
    // rank index in [0, n-1], using ceiling-based nearest-rank.
    let rank = ((p as usize * n) + 99) / 100;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_basic_distribution() {
        let fees: Vec<u64> = (1..=100).collect();
        let est = compute_percentiles(&fees);
        assert_eq!(est.samples, 100);
        assert_eq!(est.p50, 50);
        assert_eq!(est.p75, 75);
        assert_eq!(est.p90, 90);
        assert_eq!(est.p95, 95);
        assert_eq!(est.max, 100);
        assert_eq!(est.mean, 50); // (1+100)*100/2 / 100 = 50.5, truncated to 50
    }

    #[test]
    fn percentile_single_sample() {
        let est = compute_percentiles(&[42]);
        assert_eq!(est.p50, 42);
        assert_eq!(est.p95, 42);
        assert_eq!(est.max, 42);
        assert_eq!(est.mean, 42);
        assert_eq!(est.samples, 1);
    }

    #[test]
    fn percentile_handles_duplicates() {
        let fees = vec![10, 10, 10, 10, 100];
        let est = compute_percentiles(&fees);
        assert_eq!(est.p50, 10);
        assert_eq!(est.p95, 100);
        assert_eq!(est.max, 100);
        assert_eq!(est.samples, 5);
    }

    #[test]
    fn percentile_monotonic() {
        let fees: Vec<u64> = (1..=50).rev().collect(); // unsorted on input
        let est = compute_percentiles(&fees);
        assert!(est.p50 <= est.p75);
        assert!(est.p75 <= est.p90);
        assert!(est.p90 <= est.p95);
        assert!(est.p95 <= est.max);
    }

    #[test]
    fn percentile_all_zeros_is_valid() {
        // Devnet / low-congestion mainnet: every recent slot has zero priority fee.
        // The right answer is "zero", not an error.
        let est = compute_percentiles(&[0u64; 50]);
        assert_eq!(est.p50, 0);
        assert_eq!(est.p75, 0);
        assert_eq!(est.p95, 0);
        assert_eq!(est.max, 0);
        assert_eq!(est.mean, 0);
        assert_eq!(est.samples, 50);
    }

    #[test]
    fn recommended_returns_p75() {
        let est = compute_percentiles(&(1..=100).collect::<Vec<_>>());
        assert_eq!(est.recommended(), est.p75);
    }
}
