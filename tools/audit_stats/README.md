# Post-run audit statistics

`audit_stats` turns finished run directories into numbers: findings by severity,
fork-reproduction coverage, evidence integrity, tool usage, latency, and data
quality. It is stdlib-only Python, never touches the network, and never changes
an audit's exit code.

The run directory remains the source of truth. `stats/metrics.json` is derived
data: delete it and run `collect` again at any time.

```text
runs/<run_id>/
  input/run_meta.json          # written by the entrypoint on exit
  reports/final_report.json    # agent-authored, treated as untrusted input
  artifacts/                   # tool output and traces
  test/                        # Foundry harness (if any)
  stats/metrics.json           # derived by this tool
```

## Quick start

```bash
# one run
PYTHONPATH=tools python3 -m audit_stats collect runs/<run_id>

# every run under a root (skips runs that already have metrics)
PYTHONPATH=tools python3 -m audit_stats collect --runs-root runs

# a round report: JSON + Markdown + two CSVs
PYTHONPATH=tools python3 -m audit_stats report --runs-root runs --expected 120

# preview metrics without writing
PYTHONPATH=tools python3 -m audit_stats collect runs/<run_id> --print
```

`report` writes to `<runs-root>/_stats/` by default:

| File | Content |
| --- | --- |
| `summary.json` | every aggregate, machine-readable |
| `REPORT.md` | human-readable round report |
| `runs.csv` | one row per run |
| `findings.csv` | one row per finding |
| `stability.csv` | one row per repeated-target run pair |

`--since` / `--until` select runs by `run_meta.started_at`; `--backfill` collects
missing metrics on the fly; `--expected N` adds round completion.

## What is collected

### Per run (`stats/metrics.json`, `stats_schema_version: 2`)

| Block | Fields | Purpose |
| --- | --- | --- |
| `run` | status, exit_code, task_id, batch_id, model, reasoning_effort, image, prompt_sha256, timings | stratify results by model/image/batch; group stable comparisons |
| `target` | chain, address, name, verified, prompt addresses | de-duplication, verified/unverified split |
| `report` | schema grade, findings total, severity/confidence counts, max severity, hypotheses, limitations | audit yield |
| `evidence` | refs total/resolved/missing/outside, findings with evidence | claim-vs-artifact integrity |
| `fork` | harness files/tests/fuzz tests, fork signals, forge pass/fail/fuzz runs, per-finding repro linkage | fork reproduction |
| `function_coverage` | target ABI, functions seen in traces, functions referenced by the harness, uncovered list | which target surface the run actually exercised |
| `coverage` | declared vs observed for source/static/fork/fuzz | overclaim detection |
| `tooling` | slither/forge/echidna/heimdall/cast/source API/bytecode match | actual tool usage |
| `volume` | artifact/source/harness file counts and bytes | run cost proxy |
| `findings` | one compact row per finding | findings CSV without re-reading reports |
| `anomalies` | stable codes, e.g. `report_missing`, `evidence_missing` | data-quality signal |

### Per round (`summary.json`)

- **Funnel**: discovered → metrics → report present → parsed → current/legacy
  schema → evaluable → any finding → high+ finding.
- **Yield**: findings by severity and confidence, high+/critical, findings per
  evaluable run.
- **Detection**: runs with any finding and runs with high+, over the evaluable
  denominator.
- **Fork reproduction**: findings linked to a harness test, linked test passed,
  repro rate; findings in fork-backed runs and fork repro rate; harness test,
  fuzz-test, and fuzz-run counts.
- **Function coverage**: ABI functions covered by trace or harness, trace-only
  coverage, state-changing coverage, never-executed functions.
- **Stability**: for targets audited more than once, title-set overlap and
  severity drift, split into like-for-like and changed conditions.
- **Evidence**: resolution rate, missing references, findings without evidence.
- **Coverage claims**: declared-true vs observed-on-disk, over- and underclaims.
- **Tooling**: runs using each tool, slither signal count, cast traces.
- **Anomalies**: counts by code and runs affected.
- **Latency**: n, p50, p90, max, total.
- **Targets**: unique addresses, repeated audits.
- **Strata**: by model, chain, verified, schema grade.

## Definitions that matter

**Evaluable run.** Report parsed, schema current or legacy, run not failed,
report status not `blocked`. Detection rates use this denominator; the report
prints it explicitly. A run whose report is missing or unparsable is counted in
the funnel but never in a rate.

**Repro rate.** `findings with a reproduction naming a harness test that passed`
/ `all findings`. A green test only proves the test ran.

**Fork repro rate.** The same numerator, restricted to findings from runs where
a fork was detected (`createSelectFork`/`createFork`/`forkBlockNumber`,
`envString("RPC_URL")`, `--fork-url`, or `artifacts/pinned_block.json`). A local
anvil test with no fork is not a fork reproduction, so it is excluded from this
rate and visible in the gap between `repro_rate` and `fork_repro_rate`.

**Function coverage.** The share of the target's ABI functions exercised by the
run. `coverage` unions functions seen in `forge -vvvv`/`cast run` traces with
functions referenced by the harness source. It is an upper bound: a referenced
call may sit in an untaken branch. `trace_coverage` is the conservative lower
bound but only sees traces the agent saved. State-changing coverage excludes
`view`/`pure`. This is **not** line/branch coverage: the target source is often
not compiled into the Foundry project, so `forge coverage` would measure the
harness, not the target.

**Stability overlap.** For a target with two or more evaluable runs, findings are
matched one-to-one by normalized title token Jaccard (default threshold 0.6);
overlap is `matched / (A + B - matched)`. Severity delta is the severity rank of
the newer run minus the older run. Two empty finding sets count as overlap 1.0.
Pairs are flagged like-for-like when model, image, and prompt hash all match, so
a regression can be separated from a prompt change. Agreement is not truth.

**Evidence resolution.** Every `evidence` path is checked inside the run
directory. Absolute paths, `..` traversal, and symlinks that escape the run are
rejected and counted as `refs_outside_run`. Existence is checked; semantic
support for the claim is not.

**Overclaim.** The report says a coverage flag is `true` but no artifact on disk
supports it (`sources/` or Etherscan response; slither/heimdall output;
harness + forge run; fuzz test or Echidna). This is the cheapest way to catch an
agent that skipped a step it claimed to perform.

**Schema grade.** `current` (matches the report contract), `legacy` (older
shape with a findings list, e.g. pre-refactor runs), `partial`, `invalid`.

## Severity and confidence normalization

The report contract allows `critical | high | medium | low | informational`, but
old runs and model drift produce `very high`, `info`, `high (confirmed)`, etc.
Aliases are mapped; unknown values become `unknown` and raise an anomaly, so a
bad severity never silently disappears from the denominator.

`confidence: high` in legacy reports maps to `confirmed`.

## Trust and safety

The report is model-authored. The collector:

- parses JSON only, never executes report content;
- caps text reads at 8 MB per file;
- confines evidence paths to the run directory (realpath containment);
- records schema errors instead of failing;
- never writes outside `stats/`;
- writes atomically (temp file + `os.replace`), so a killed collector cannot
  leave a half-written metrics file;
- makes no network calls and starts no subprocesses.

A malicious agent can still forge its own findings; the metrics measure internal
consistency and effort, not truth. Severity is a triage signal.

## Container and k3s integration

The runtime image copies this package to `/opt/agent-audit/tools`. On exit, the
entrypoint runs:

```bash
python3 -m audit_stats collect "$RUN_DIR" --quiet
```

`AGENT_AUDIT_STATS=0` disables it. Failure is logged to
`runs/<run_id>/logs/stats.log` and never changes the audit exit code.

In k3s, `stats/metrics.json` lands on the node-local hostPath with the rest of
the run. Aggregation is a separate, pull-based step:

```bash
# single node / shared volume
PYTHONPATH=tools python3 -m audit_stats report \
  --runs-root /var/lib/agent-audit/runs --expected 500 \
  --out /var/lib/agent-audit/reports/round-20261027

# multi node: copy only the metrics files, then aggregate centrally
for node in node1 node2 node3; do
  rsync -a --include='*/' --include='stats/' --include='stats/metrics.json' \
    --exclude='*' "root@${node}:/var/lib/agent-audit/runs/" "/central/runs/"
done
PYTHONPATH=tools python3 -m audit_stats report --runs-root /central/runs --expected 500
```

The aggregator only needs `*/stats/metrics.json`. It never reads live run
directories across nodes, so a half-written artifact on one node cannot corrupt
a round report. Runs with no metrics appear in the funnel and in
`missing_sample`; `--backfill` recomputes them when the run directories are
reachable.

A Kubernetes CronJob (single node) or DaemonSet (multi node) can run the same
two commands. Redis, Postgres, and object storage are optional; none is
required for correctness.

## Explicitly not measured (yet)

| Gap | Why | Suggested next step |
| --- | --- | --- |
| Tokens and cost | DSH does not write usage into the run directory; the session log lives under `DSH_HOME` and is lost with the Pod | Export a small `stats/usage.json` from the DSH session before exit |
| Line/branch coverage | `forge coverage` instruments compiled source; fork-executed bytecode is not instrumented, so most runs would measure the harness | Only for runs that compile and deploy the target source; write `stats/coverage.json` behind a whitelist |
| Precision / recall | Requires human labels or disclosed incidents | Join with a ground-truth table; keep severity out of the headline |
| Cross-run dedup | Same known issue reported on many targets | Boilerplate list + title clustering |

## Tests

```bash
cd tools
python3 -m unittest discover -s audit_stats/tests -t . -v
```

Covers severity/confidence normalization, evidence path containment (including
symlink escape), forge log merging, fork/repro linkage, trace-based function
coverage, title Jaccard and severity drift, legacy report handling, aggregate
denominators, and output rendering.
