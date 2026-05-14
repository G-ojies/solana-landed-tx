//! Python bindings for solana-landed-tx.
//!
//! Exposes a blocking `FeeEstimator` class. Internally bridges to tokio so
//! Python users don't have to deal with asyncio for a single fee-estimate call.
//! If you're in an asyncio context, wrap calls with `loop.run_in_executor(None, ...)`.

use std::str::FromStr;
use std::sync::Arc;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use ::solana_landed_tx::FeeEstimator as RustFeeEstimator;
use solana_sdk::pubkey::Pubkey;
use tokio::runtime::Runtime;

/// Result of a priority fee estimate. All values in µLamports per compute unit.
#[pyclass(frozen, get_all)]
#[derive(Clone)]
struct FeeEstimate {
    p50: u64,
    p75: u64,
    p90: u64,
    p95: u64,
    max: u64,
    mean: u64,
    samples: usize,
}

#[pymethods]
impl FeeEstimate {
    fn __repr__(&self) -> String {
        format!(
            "FeeEstimate(p50={}, p75={}, p90={}, p95={}, max={}, mean={}, samples={})",
            self.p50, self.p75, self.p90, self.p95, self.max, self.mean, self.samples
        )
    }

    /// p75 — sensible mid-aggressive starting point.
    fn recommended(&self) -> u64 {
        self.p75
    }
}

/// Priority fee estimator.
///
/// Wraps a Solana RPC client and queries `getRecentPrioritizationFees`.
#[pyclass]
struct PyFeeEstimator {
    rpc_url: String,
}

#[pymethods]
impl PyFeeEstimator {
    /// Create a new estimator pointed at the given Solana RPC URL.
    #[new]
    fn new(rpc_url: String) -> Self {
        Self { rpc_url }
    }

    /// Query recent prioritization fees and return percentile-based estimates.
    ///
    /// `writable_accounts` (optional): list of base58 pubkey strings to scope
    /// the query to slots that wrote to those accounts. Pass None or [] for a
    /// global estimate.
    ///
    /// Blocks the calling thread. Suitable for sync scripts; for asyncio use
    /// `loop.run_in_executor(None, estimator.estimate)`.
    #[pyo3(signature = (writable_accounts = None))]
    fn estimate(&self, writable_accounts: Option<Vec<String>>) -> PyResult<FeeEstimate> {
        let accounts: Vec<Pubkey> = writable_accounts
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                Pubkey::from_str(&s).map_err(|e| {
                    PyValueError::new_err(format!("invalid pubkey '{s}': {e}"))
                })
            })
            .collect::<PyResult<_>>()?;

        let rpc_url = self.rpc_url.clone();
        let runtime = Runtime::new()
            .map_err(|e| PyRuntimeError::new_err(format!("tokio runtime: {e}")))?;

        let est = runtime
            .block_on(async move {
                let rpc = Arc::new(
                    solana_client::nonblocking::rpc_client::RpcClient::new(rpc_url),
                );
                let estimator = RustFeeEstimator::new(rpc);
                estimator.estimate(&accounts).await
            })
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        Ok(FeeEstimate {
            p50: est.p50,
            p75: est.p75,
            p90: est.p90,
            p95: est.p95,
            max: est.max,
            mean: est.mean,
            samples: est.samples,
        })
    }
}

#[pymodule]
fn solana_landed_tx(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<FeeEstimate>()?;
    m.add_class::<PyFeeEstimator>()?;
    // Expose under a friendlier name.
    m.add("FeeEstimator", m.getattr("PyFeeEstimator")?)?;
    Ok(())
}
