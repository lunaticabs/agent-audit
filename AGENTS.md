# Repository Guidelines

- tools you need is given in this repo, see `.agents/skills` to 
- Trust the repository-local `.env` for `AGENT_AUDIT_*` configuration.
- Prefix every `uv` command with `UV_CACHE_DIR=/tmp/uv-cache`.
- Execute repository CLI steps sequentially for a single `run_id`.
- Treat every generated file as review material, not as a final audit conclusion.
- Slither is not the verdict. Slither is only a hint source. Do not produce a final report that mainly restates Slither warnings.
- You must actively try Foundry, cast, anvil, and Echidna where relevant. If a serious hypothesis exists, try to reproduce it or disprove it before finalizing.
- Do not conclude any thing is secure unless you have specifically examined and validated it.

## Tool Skills

Use these repo skills in `.agents/skills/` as needed:

- `$audit-workspace`
- `$audit-source-fetch`
- `$audit-ir-builder`
- `$audit-dependency-scan`
- `$audit-slither`
- `$audit-materials`
- `$audit-echidna`
- `$foundry-forge`
- `$foundry-cast`
- `$foundry-anvil`
- `$foundry-chisel`

## Recommended Workflow

The repository CLI can prepare almost all materials you need.

Suggested order:

1. `$audit-workspace`
2. `$audit-source-fetch`
3. `$audit-ir-builder`
4. `$audit-dependency-scan`
5. `$audit-materials`

After that:

- inspect `reports/materials_manifest.json`
- inspect raw evidence files
- decide whether use tools like: Slither, Echidna, Forge, Cast, or Anvil
- if you run direct tools, save their artifacts under the same `runs/<run_id>/artifacts/` tree
- reuse `$audit-materials` if you want the manifest to list those optional artifacts

`$audit-source-fetch` prepares the Slither-compatible workspace automatically when source fetching succeeds.

Repository-side findings, when present, live in `artifacts/dependency_findings.json`.

Finally:

if you think you have identified the real vulnerabilities, or the contract is safe, write a JSON report and save it under `runs/<run_id>/reports/final_report.json`.

**Important:**

Any reported finding must be backed by a concrete artifact in runs/<run_id>/.
Do not report a finding unless you can cite the exact supporting artifact file(s).

Acceptable support includes:
- source-level code references
- repository-generated artifacts
- direct tool artifacts you saved under runs/<run_id>/artifacts/
- reproducible test/fuzz outputs

If a claim has no artifact support, label it as an unconfirmed hypothesis, not a finding.
Do not include unsupported tool-usage claims, on-chain state claims, or conclusions in the final report.
