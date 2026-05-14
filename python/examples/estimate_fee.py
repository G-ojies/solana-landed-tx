"""Print the current priority fee distribution for a given Solana RPC.

Usage:
    python examples/estimate_fee.py
    python examples/estimate_fee.py https://api.mainnet-beta.solana.com
    python examples/estimate_fee.py https://api.mainnet-beta.solana.com <PUBKEY>
"""

import sys

from solana_landed_tx import FeeEstimator

DEFAULT_RPC = "https://api.mainnet-beta.solana.com"
USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"


def main() -> None:
    rpc_url = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_RPC
    writable = sys.argv[2] if len(sys.argv) > 2 else USDC_MINT

    print(f"RPC:     {rpc_url}")
    print(f"Scoped to writable account: {writable}")

    est = FeeEstimator(rpc_url)
    result = est.estimate([writable])

    print(f"samples: {result.samples}")
    print(f"p50:     {result.p50} µLamports/CU")
    print(f"p75:     {result.p75} µLamports/CU")
    print(f"p90:     {result.p90} µLamports/CU")
    print(f"p95:     {result.p95} µLamports/CU")
    print(f"max:     {result.max} µLamports/CU")
    print(f"mean:    {result.mean} µLamports/CU")

    cu_limit = 200_000
    lamports = (result.p75 * cu_limit) // 1_000_000
    print(
        f"\nAt p75 with a {cu_limit} CU budget, "
        f"you'd pay {lamports} lamports in priority fees."
    )


if __name__ == "__main__":
    main()
