---
name: audit-source-fetch
description: Fetch verified source and dependency source bundles for an existing run. Use when the run workspace exists and source materials are still missing.
---

# Audit Source Fetch

Fetch verified source:

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit fetch-source --run-id <run_id>
```

This step uses an Etherscan V2-compatible API and switches chains by `chainid`.

Practical notes:

- You can usually keep one V2 API base URL and vary only the requested chain.
- Chain aliases are normalized, so forms like `arb-sepolia`, `arb_sepolia`, and `Arbitrum Sepolia` resolve to the same `chainid`.
- Common supported aliases include `eth`, `base`, `arb`, `op`, `polygon`, `bsc`, `avax`, `linea`, `blast`, `scroll`, `mantle`, `gnosis`, `celo`, `zksync`, plus common testnets such as `sepolia`, `base-sepolia`, `arb-sepolia`, `op-sepolia`, `amoy`, `fuji`, `linea-sepolia`, and `scroll-sepolia`.

Inspect first:

- `artifacts/source_bundle.json`
- `artifacts/source_provider_response.json`
- `sources/`
