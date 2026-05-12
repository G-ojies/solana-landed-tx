//! Reliable Solana transaction landing.
//!
//! Two pieces:
//! - [`FeeEstimator`]: pulls recent prioritization fees from any RPC and computes
//!   percentile-based estimates.
//! - `send_and_confirm_with_retry` (coming next): a send primitive that attaches a
//!   priority fee, polls confirmation, and bumps the fee on retry.
//!
//! ## Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use solana_client::nonblocking::rpc_client::RpcClient;
//! use solana_landed_tx::FeeEstimator;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = Arc::new(RpcClient::new("https://api.mainnet-beta.solana.com".into()));
//! let estimator = FeeEstimator::new(rpc);
//! let estimate = estimator.estimate(&[]).await?;
//! println!("p75 priority fee: {} micro-lamports/CU", estimate.p75);
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod estimator;

pub use error::{LandedTxError, Result};
pub use estimator::{FeeEstimate, FeeEstimator};
