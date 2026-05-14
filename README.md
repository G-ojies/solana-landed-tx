# solana-landed-tx

Reliable Solana transaction landing - priority fee estimation and a battle-tested
send-and-confirm primitive, with a public benchmark of which strategies actually work.

> **Status:** v0.1 alpha. Estimator, retry sender, and Python bindings all
> functional. Tested against mainnet + a local validator. Benchmark harness next.

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

## Quick start (Python)

```bash
pip install maturin
git clone https://github.com/G-ojies/solana-landed-tx
cd solana-landed-tx/python
python -m venv .venv && source .venv/bin/activate
maturin develop --release
```

```python
from solana_landed_tx import FeeEstimator

est = FeeEstimator("https://api.mainnet-beta.solana.com")

# Global estimate
result = est.estimate()
print(result)
# FeeEstimate(p50=0, p75=0, p90=10000, p95=65865, max=178444, ...)

# Or scope to specific writable accounts for a more relevant signal:
usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
result = est.estimate([usdc])
print(f"p95: {result.p95} µLamports/CU")
print(f"recommended: {result.recommended()} µLamports/CU")
```

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

### 1. Fee estimation against real mainnet

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

### 2. End-to-end send through the retry sender

```
$ solana-test-validator --reset &
$ cargo run --example send_self_transfer

RPC: http://127.0.0.1:8899
payer: cUnZVHAVG4qnvBoYQUb26iy3obpseComEdmCeZ4CD8X
requesting airdrop of 1000000000 lamports...
airdrop confirmed: 16hYvsEP4ZUQMMkaZPC6QTePx6z3A9T48GLbzSYfK1Lj6xb7nAKXivbpivhm68cxB8rcUrgg3aeM234yNmjSHBx
balance: 1000000000 lamports
sending self-transfer (1000 lamports) via retry sender...

=== LANDED ===
signature:      2R2vtuZRNz12zFRoc9WpmwXSTo7cbGd8bo3Gd6NvxsNNLgZamHv7zFQpy5a2fcCx9P3BfseS9xGqDmUcH7QPcaS3
attempts:       1
elapsed:        506 ms
cu_price:       1000 µLamports/CU
priority paid:  200 lamports
```

506 ms from airdrop-confirm to landed self-transfer on a fresh validator. Same code path runs against devnet/mainnet — just point at a different RPC.

## Testing

Unit tests (pure math, no network):

```
cargo test
```

Live integration test against a local validator:

```
# Terminal 1
solana-test-validator --reset

# Terminal 2
cargo test --test integration_send -- --ignored --nocapture
```

Or point at any RPC (devnet, custom localnet, etc.):

```
SOLANA_LANDED_TX_TEST_RPC=https://api.devnet.solana.com \
    cargo test --test integration_send -- --ignored --nocapture
```

## Roadmap

- [x] Priority fee estimator with percentile breakdowns
- [x] Account-scoped fee queries
- [x] Unit tests for percentile math
- [x] Live mainnet/devnet example
- [x] `send_and_confirm_with_retry` with fee bumping
- [x] Telemetry (attempts, time-to-land, priority lamports paid)
- [x] Live integration test against `solana-test-validator`
- [x] Python bindings via PyO3 + maturin
- [ ] Pluggable RPC backends (Helius, Triton)
- [ ] Expose `Sender` to Python (currently only `FeeEstimator`)
- [ ] Reproducible mainnet benchmark of 5 strategies
- [ ] Public results dashboard

## License

MIT.
