# solana-landed-tx

Reliable Solana transaction landing — priority fee estimation and a battle-tested
send-and-confirm primitive, with a public benchmark of which strategies actually work.

> **Status:** v0.1 alpha. Estimator is functional and tested against mainnet.
> Retry sender and Python bindings in progress.

## The problem

Landing transactions on Solana is currently a dark art. Devs either:

- Send with no priority fee and watch transactions silently drop during congestion,
- Hardcode an arbitrary fee (usually wildly over- or under-paying), or
- Pay for closed-source premium fee APIs they can't audit.

There's no clean, open-source library that pulls fee data from any public RPC and
gives you a primitive that *just lands*.

## What this is

A single Rust crate (with Python bindings in progress) that does two things:

1. **Estimate priority fees** from any RPC's `getRecentPrioritizationFees`, with
   percentile breakdowns (p50/p75/p90/p95/max).
2. **Send and confirm with retry** (coming next): attach a fee, send, poll, bump
   the fee if not landed, repeat. With telemetry.

A reproducible mainnet benchmark of all strategies will be published when the
sender lands.

## Quick start (Rust)

```toml
[dependencies]
solana-landed-tx = "0.1"
```

```rust
use std::sync::Arc;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_landed_tx::FeeEstimator;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Arc::new(RpcClient::new("https://api.mainnet-beta.solana.com".into()));
    let estimator = FeeEstimator::new(rpc);

    // Pass `&[]` for a global estimate, or specific writable accounts to scope
    // the query to slots that touched them (much more relevant signal).
    let est = estimator.estimate(&[]).await?;

    println!("p75: {} µLamports/CU", est.p75);
    println!("p95: {} µLamports/CU", est.p95);
    Ok(())
}
```

## Live demo

```
$ cargo run --example estimate_fee -- https://api.mainnet-beta.solana.com

RPC:     https://api.mainnet-beta.solana.com
Scoped to writable account: EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
samples: 150
p50:     0 µLamports/CU
p75:     0 µLamports/CU
p90:     67 µLamports/CU
p95:     63594 µLamports/CU
max:     1900000 µLamports/CU
mean:    42519 µLamports/CU
```

That 30000x spread between p90 and max is exactly why naive fee strategies fail.

## Roadmap

- [x] Priority fee estimator with percentile breakdowns
- [x] Account-scoped fee queries
- [x] Unit tests for percentile math
- [x] Live mainnet/devnet example
- [ ] `send_and_confirm_with_retry` with fee bumping
- [ ] Pluggable RPC backends (vanilla, Helius, Triton)
- [ ] Telemetry (attempts, total cost, time-to-land)
- [ ] Python bindings via PyO3 + maturin
- [ ] Reproducible mainnet benchmark of 5 strategies
- [ ] Public results dashboard

## License

MIT.
