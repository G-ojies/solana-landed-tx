# solana-landed-tx benchmark harness

Reproducible measurement of how 6 fee strategies compare on Solana, by landing rate, time-to-land, and total priority fees paid.

## Strategies tested

| Strategy | Description |
|---|---|
| `no_fee` | No `ComputeBudgetInstruction`. What naive senders do. |
| `fixed_low` | Fixed 1,000 µLamports/CU. |
| `fixed_high` | Fixed 50,000 µLamports/CU. |
| `p75` | Single-shot at the library's p75 estimate. |
| `p95` | Single-shot at the library's p95 estimate. |
| `library_retry` | Full `send_and_confirm_with_retry` starting at p75, bumping 1.5x up to 5 attempts. |

## Usage

### Localnet smoke test (no SOL needed)

```
solana-test-validator --reset &
cargo run --release -p solana-landed-tx-benchmark -- \
    --rpc http://127.0.0.1:8899 \
    --txs-per-strategy 3 \
    --output bench-localnet
```

On localnet there is no congestion, so every strategy lands at 100%. This run only verifies the framework — see `example-output/` for what that looks like.

### Real mainnet benchmark (requires funded keypair)

```
cargo run --release -p solana-landed-tx-benchmark -- \
    --rpc https://api.mainnet-beta.solana.com \
    --keypair ~/.config/solana/id.json \
    --txs-per-strategy 50 \
    --output bench-mainnet
```

Cost estimate for `--txs-per-strategy 50` (300 txs total): roughly 0.0015 SOL base + priority fees (varies by congestion). Budget ~0.05 SOL to be safe across multiple runs.

## Output

| File | Contents |
|---|---|
| `<strategy>.csv` | One row per transaction sent under that strategy. |
| `raw.csv` | All transactions across all strategies, concatenated. |
| `summary.csv` | One row per strategy: landing rate, mean / median time-to-land, mean / total priority fees paid. |

### Per-tx columns

`strategy, tx_index, signature, sent_ms_from_start, landed_ms_from_start, time_to_land_ms, cu_price_micro_lamports, priority_lamports_paid, attempts, landed`

### Summary columns

`strategy, txs, landed, landing_rate_pct, mean_time_to_land_ms, median_time_to_land_ms, mean_priority_lamports, total_priority_lamports`

## Caveat: strategy ordering can confound

Strategies run sequentially, and `getRecentPrioritizationFees` reflects the last ~150 slots of on-chain activity — which includes earlier strategies' transactions. On a low-traffic chain (e.g. localnet) this means later strategies' percentile estimates are skewed by earlier strategies' fees. On mainnet this is negligible because each strategy's contribution is a drop in the bucket of millions of unrelated transactions.

If you want fully independent strategy comparison on a low-traffic network, run each strategy in its own benchmark invocation with `--txs-per-strategy 50` and a single-strategy filter (TODO: add `--only <strategy>` flag).

## Example output

See `example-output/` for a real run of the localnet smoke test.
