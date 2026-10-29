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
  artifacts/replay/            # our own post-exit replay logs + replay.json
  test/                        # Foundry harness (if any)
  stats/metrics.json           # derived by this tool
```

`replay` is the one stage that starts subprocesses: it re-runs the claimed tests
after the agent exits so the PASS the metrics trust is produced by us, not read
from the agent's artifacts. It has its own timeout budget and is best-effort.
`collect` and `report` remain offline and subprocess-free.

## Quick start

```bash
# rebuild fork evidence after the agent exits (usually done by the entrypoint)
PYTHONPATH=tools python3 -m audit_stats replay runs/<run_id>

# one run
PYTHONPATH=tools python3 -m audit_stats collect runs/<run_id>

# every run under a root (skips runs that already have metrics)
PYTHONPATH=tools python3 -m audit_stats collect --runs-root runs

# a round report: JSON + Markdown + two CSVs
PYTHONPATH=tools python3 -m audit_stats report --runs-root runs --expected 120

# findings that need a human look: TRIAGE.md + triage.csv
PYTHONPATH=tools python3 -m audit_stats triage --runs-root runs

# preview metrics without writing
PYTHONPATH=tools python3 -m audit_stats collect runs/<run_id> --print
```

`triage` lists findings whose reproduction is missing, did not reproduce, is
only present in the agent's log, or is not state-sensitive, plus runs with
integrity anomalies. Reasons, most urgent first:

| Reason | Means |
| --- | --- |
| `not_reproduced` | names a test, and that test did not pass |
| `unattested` | the PASS is only in the agent's log, although a replay ran |
| `log_only` | the PASS is only in the agent's log because no replay ran |
| `evidence_missing` | no evidence reference, or at least one that does not resolve |
| `no_reproduction` | names no test at all |
| `vacuity_risk` | attested, but it passed at both blocks, so the test may assert nothing |
| `unknown_severity` | severity could not be normalized |

Rows are sorted by severity first, reason second: a high finding proven only by
the agent's log is more urgent than a low finding that simply did not reproduce.
The list is advisory and its absence is not a clean bill of health.

`report` writes to `<runs-root>/_stats/` by default:

| File | Content |
| --- | --- |
| `summary.json` | every aggregate, machine-readable |
| `REPORT.md` | human-readable round report |
| `runs.csv` | one row per run |
| `findings.csv` | one row per finding |
| `stability.csv` | one row per repeated-target run pair |

`triage` writes `TRIAGE.md` and `triage.csv` to the same directory.

`--since` / `--until` select runs by `run_meta.started_at`; `--backfill` collects
missing metrics on the fly; `--expected N` adds round completion.

## What is collected

### Per run (`stats/metrics.json`, `stats_schema_version: 3`)

| Block | Fields | Purpose |
| --- | --- | --- |
| `run` | status, exit_code, task_id, batch_id, model, reasoning_effort, image, prompt_sha256, timings | stratify results by model/image/batch; group stable comparisons |
| `target` | chain, address, name, verified, prompt addresses | de-duplication, verified/unverified split |
| `report` | schema grade, findings total, severity/confidence counts, max severity, hypotheses, limitations | audit yield |
| `evidence` | refs total/resolved/missing/outside, findings with evidence | claim-vs-artifact integrity |
| `fork` | harness files/tests/fuzz tests, fork marker sites, pinned block, forge pass/fail/fuzz runs, replay status, per-finding repro linkage/attestation/state sensitivity | fork reproduction |
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
  repro rate; how many passed under the post-exit replay (`attested`) and how
  many are state-sensitive; findings in fork-backed runs and fork repro rate;
  runs with a replay and runs whose fork is corroborated at runtime; harness
  test, fuzz-test, and fuzz-run counts.
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

**Fork markers.** A marker only counts when it appears inside a function body —
`testXxx`, `setUp`, or a helper. The same string in a comment, a string literal,
or at file scope is recorded in `fork_markers_comment_only` /
`fork_markers_outside_functions` and raises `fork_marker_not_in_test_body`. The
old implementation was a substring match over the whole file, so a comment was
enough to label a run fork-backed.

**Replay and attestation.** `replay` re-runs the claimed tests after the agent
exits and writes `artifacts/replay/replay.json` plus one log per run. The
collector prefers that record over the agent's own forge logs:
`findings_repro.passed` counts a PASS from either source, and `attested` counts
only the ones the replay produced itself. A finding whose test passed in the
agent's log but not in the replay raises `repro_unattested`; a test the replay
ran and failed raises `repro_contradicted_by_replay`. Replay records the harness
file sha256 so the PASS is bound to a version of the test.

**State sensitivity (diagnostic).** The replay also runs at `block - delta`
(default 7200). A test that passes at the pinned block and fails there is
`state_sensitive`. This is deliberately **not** part of the repro rate: a logic
bug present at both blocks is reproduced correctly but not state-sensitive, so
the diagnostic under-counts. It answers "does the assertion read chain state",
not "is the finding real".

**Fork verified.** `fork_backed` is static (marker in a function body or a
pinned block file). `fork_verified` additionally needs runtime corroboration:
a replay row, a pinned block number, or a fork signal in a parsed log.

**Evidence-integrity anomalies.** These fire when a claim and what can be
recomputed disagree:

| Code | Means |
| --- | --- |
| `repro_contradicted_by_replay` | the agent's log says PASS, our replay says FAIL |
| `repro_unattested` | the PASS exists only in the agent's log; the replay did not cover it |
| `forge_log_test_unknown` | a `[PASS]` line names a test that exists nowhere in the harness, so the log cannot be `forge test` output |
| `fork_marker_not_in_test_body` | the only fork markers are in comments or at file scope |
| `fork_replay_failed` / `fork_replay_skipped` | the replay ran and failed, or was skipped with a reason |

They falsify a claim; their absence does not prove honesty. A test that asserts
something true at every block is replayed faithfully and passes.

**Function coverage.** The share of the target's ABI functions exercised by the
run. `coverage` unions functions seen in `forge -vvvv`/`cast run` traces with
functions referenced by the harness source. It is an upper bound: a referenced
call may sit in an untaken branch. `trace_coverage` is the conservative lower
bound but only sees traces the agent saved. State-changing coverage excludes
`view`/`pure`. This is **not** line/branch coverage: the target source is often
not compiled into the Foundry project, so `forge coverage` would measure the
harness, not the target.

ABI selection is scored, not "largest wins": agents save several ABIs (target,
owner, vault, upstream metadata), and a large unrelated file can otherwise
dominate the denominator. Candidates are ranked by overlap with trace/harness
calls, then an explicit `sources/abi.json`, target address/name hints, and
finally size. Bare JSON lists only count when their entries look like ABI items,
which keeps GitHub tag lists out.

**Stability overlap.** For a target with two or more evaluable runs, findings are
matched one-to-one. A pair is anchored by the `function` field when the function
sets overlap and title similarity is at least 0.35; without a function anchor the
title similarity must reach 0.5. Title similarity is the maximum of token
Jaccard, overlap coefficient, and sequence ratio, because model-authored titles
are reworded heavily between runs. Overlap is
`matched / (A + B - matched)`; severity delta is the newer run minus the older
run. Two empty finding sets count as overlap 1.0. Pairs are flagged like-for-like
when model, image, and prompt hash all match, so a regression can be separated
from a prompt change. `stability.csv` records whether each pair was anchored by
function or title. Agreement is not truth.

On a real WETH9 pair (local run vs the k3s run, same prompt and model) the
reworded findings matched 4 of 6 union findings (66.7%) with two +1 severity
upgrades. Pure title Jaccard on the same pair was 0.22, which is why the
function anchor exists.

**Evidence resolution.** Every `evidence` path is checked inside the run
directory. Line suffixes (`:12`, `:12-34`, `:12-34,56-78`), `#L12-L30`, and
trailing parenthetical notes are stripped first. Absolute paths, `..` traversal,
and symlinks that escape the run are rejected and counted as
`refs_outside_run`. Existence is checked; semantic support for the claim is not.

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
- writes only `stats/` and never mutates audit evidence;
- writes atomically (temp file + `os.replace`), so a killed collector cannot
  leave a half-written metrics file;
- makes no network calls and starts no subprocesses.

The replay stage does run code, and it is explicit about what that buys:

- it runs a fixed argv (`forge test --fork-url … --match-test …`) with `cwd` set
  to a Foundry project inside the run directory, no shell, and a per-run timeout;
- it drops `DEEPSEEK_API_KEY` from the child environment and redacts the RPC URL
  (path and query dropped) in the log header;
- its output lives under `artifacts/replay/`, next to the evidence it replaces,
  and `stats/` stays purely derived;
- it runs after the agent process exits, so the agent cannot read or rewrite the
  result — but it is still the same container: this is a *republished* log, not a
  separate security domain.

A malicious agent can still write findings it cannot support; the metrics measure
internal consistency and effort, not truth. Severity is a triage signal.

## Container and k3s integration

The runtime image copies this package to `/opt/agent-audit/tools`. On exit, the
entrypoint runs:

```bash
python3 -m audit_stats replay "$RUN_DIR"   # agent has exited; writes artifacts/replay/
python3 -m audit_stats collect "$RUN_DIR" --quiet
```

`AGENT_AUDIT_STATS=0` disables both. `AGENT_AUDIT_REPLAY=0` disables only the
replay. `AGENT_AUDIT_REPLAY_TIMEOUT_MS` (default 900000) is the total replay
budget and `AGENT_AUDIT_REPLAY_BLOCK_DELTA` (default 7200) is the gap between the
two blocks. Failures are logged to `runs/<run_id>/logs/replay.log` and
`logs/stats.log` and never change the audit exit code.

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
symlink escape), forge log merging, fork-marker classification (comment, file
scope, `setUp`, test body), replay ingestion and contradiction, two-block state
sensitivity, replay discovery/skip paths, trace-based function coverage,
function-anchored title matching, severity drift, legacy report handling,
aggregate denominators, and output rendering.
