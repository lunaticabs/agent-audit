---
name: foundry-forge
description: Use Foundry forge directly in the benchmark-provided EVMbench audit repository.
---

# EVMbench Forge

Run Forge from the local benchmark repository. Do not expect a production
`runs/<run_id>/foundry_project`.

Start by checking for Foundry configuration:

```bash
find . -maxdepth 4 -type f \( -name 'foundry.toml' -o -name 'remappings.txt' -o -name '*.t.sol' -o -name '*.s.sol' \)
```

Common local workflows:

```bash
forge build
```

```bash
forge test
```

```bash
forge test --match-contract <ContractTest> --match-test <test_name> -vvvv
```

If validating a suspected issue requires a custom harness, put it in a local
project-appropriate test directory or an optional intermediate directory such as
`runs/evmbench/forge/`. Keep it based only on benchmark-local source and test
data.

Save useful raw output under `LOGS_DIR`:

```bash
mkdir -p "${LOGS_DIR:-logs}"
forge test -vvvv > "${LOGS_DIR:-logs}/forge_test.txt" 2>&1
```

Audit guidance:

- Prefer project-native tests and targeted traces for confirmation.
- Do not use `--broadcast`.
- Do not default to real RPC forks. Use local Anvil or benchmark-local tests
  unless the benchmark repository explicitly provides and requires another
  source.
- Do not run `agent-audit prepare-tooling` or `aggregate-materials`; those are
  production pipeline steps.

Official docs:

- https://getfoundry.sh/forge
- https://getfoundry.sh/forge/tests/overview/
