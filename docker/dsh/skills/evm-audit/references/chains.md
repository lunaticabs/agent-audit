# Supported chains

Chain aliases are case-insensitive. Normalization lowercases the input and
removes everything that is not an ASCII letter or digit, so `Arbitrum-One`
becomes `arbitrumone`. Prefer the canonical alias, and pass the chain ID to
Etherscan v2 as `chainid=<id>`.

| Chain | Chain ID | Accepted aliases |
| --- | ---: | --- |
| Ethereum mainnet | 1 | `eth`, `ethereum`, `mainnet` |
| Holesky | 17000 | `holesky` |
| Hoodi | 560048 | `hoodi` |
| Sepolia | 11155111 | `sepolia` |
| BNB Smart Chain | 56 | `bsc`, `bnb`, `bnbsmartchain`, `binancesmartchain` |
| BNB testnet | 97 | `bsctestnet`, `bnbtestnet` |
| Polygon PoS | 137 | `polygon`, `matic`, `polygonmainnet` |
| Polygon Amoy | 80002 | `amoy`, `polygonamoy` |
| Base | 8453 | `base`, `basemainnet` |
| Base Sepolia | 84532 | `basesepolia` |
| Arbitrum One | 42161 | `arb`, `arbone`, `arbitrum`, `arbitrumone` |
| Arbitrum Nova | 42170 | `arbnova`, `arbitrumnova` |
| Arbitrum Sepolia | 421614 | `arbsepolia`, `arbitrumsepolia` |
| OP Mainnet | 10 | `op`, `optimism`, `opmainnet` |
| OP Sepolia | 11155420 | `opsepolia`, `optimismsepolia` |
| Avalanche C-Chain | 43114 | `avalanche`, `avax`, `avalanchecchain` |
| Avalanche Fuji | 43113 | `fuji`, `avalanchefuji` |
| Linea | 59144 | `linea` |
| Linea Sepolia | 59141 | `lineasepolia` |
| Blast | 81457 | `blast` |
| Blast Sepolia | 168587773 | `blastsepolia` |
| Scroll | 534352 | `scroll` |
| Scroll Sepolia | 534351 | `scrollsepolia` |
| Mantle | 5000 | `mantle` |
| Mantle Sepolia | 5003 | `mantlesepolia` |
| Gnosis | 100 | `gnosis`, `xdai` |
| Celo | 42220 | `celo` |
| Celo Sepolia | 11142220 | `celosepolia` |
| zkSync Era | 324 | `zksync`, `zksyncmainnet` |
| zkSync Sepolia | 300 | `zksyncsepolia` |
| opBNB | 204 | `opbnb` |
| opBNB testnet | 5611 | `opbnbtestnet` |
| Moonbeam | 1284 | `moonbeam` |
| Moonriver | 1285 | `moonriver` |
| Moonbase Alpha | 1287 | `moonbasealpha` |
| BitTorrent Chain | 199 | `bittorrent`, `btt` |
| BitTorrent testnet | 1029 | `btttestnet` |
| Fraxtal | 252 | `fraxtal` |
| Fraxtal Hoodi | 2523 | `fraxtalhoodi` |
| Sonic | 146 | `sonic` |
| Sonic testnet | 14601 | `sonictestnet` |
| Sei | 1329 | `sei` |
| Sei testnet | 1328 | `seitestnet` |
| Taiko | 167000 | `taiko` |
| Taiko Hoodi | 167013 | `taikohoodi` |
| Unichain | 130 | `unichain` |
| Unichain Sepolia | 1301 | `unichainsepolia` |
| World Chain | 480 | `world`, `worldchain` |

Etherscan v2 covers most of these networks with one API key. If a `chainid`
returns an unsupported-chain error, fall back to `RPC_URL` plus the chain's
official explorer. Do not substitute a different network silently.
