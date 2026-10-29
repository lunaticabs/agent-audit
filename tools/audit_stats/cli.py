from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

from . import __version__
from .aggregate import aggregate, load_metrics, write_outputs
from .collect import collect_run, metrics_path, write_run_metrics
from .common import format_pct, format_seconds, iter_run_dirs
from .replay import run_replay, write_replay
from .triage import console_summary, triage, write_outputs as write_triage


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="audit_stats",
        description=(
            "Derive post-run statistics from agent-audit run directories. "
            "`collect` writes runs/<run_id>/stats/metrics.json; `triage` lists findings that "
            "need a human look; `report` aggregates them."
        ),
    )
    parser.add_argument("--version", action="version", version="audit_stats {}".format(__version__))
    subparsers = parser.add_subparsers(dest="command", required=True)

    collect = subparsers.add_parser("collect", help="derive metrics for one or many runs")
    collect.add_argument("run_dir", nargs="?", help="one run directory")
    collect.add_argument("--runs-root", help="collect every run under this root")
    collect.add_argument("--force", action="store_true", help="rewrite existing metrics")
    collect.add_argument("--print", dest="print_json", action="store_true", help="print metrics JSON")
    collect.add_argument("--now", help="override collection timestamp (tests, backfills)")
    collect.add_argument("--quiet", action="store_true")

    replay = subparsers.add_parser(
        "replay",
        help="re-run the claimed fork tests after the agent exits",
        description=(
            "Rebuild fork-reproduction evidence from outside the agent's own logs: re-run the "
            "tests named by each finding, at the pinned block and at block minus delta."
        ),
    )
    replay.add_argument("run_dir", help="one run directory")
    replay.add_argument("--rpc-url", help="RPC endpoint (default: $RPC_URL)")
    replay.add_argument("--block", type=int, help="fork block (default: pinned block file, else chain head)")
    replay.add_argument(
        "--delta", type=int, help="blocks between the two runs (default: $AGENT_AUDIT_REPLAY_BLOCK_DELTA)"
    )
    replay.add_argument(
        "--budget-ms", type=int, help="total budget (default: $AGENT_AUDIT_REPLAY_TIMEOUT_MS)"
    )
    replay.add_argument("--json", dest="print_json", action="store_true", help="print the replay record")
    replay.add_argument("--quiet", action="store_true")

    triage = subparsers.add_parser(
        "triage",
        help="list findings that need a human look",
        description=(
            "List findings whose reproduction is missing, not reproduced, only present in the "
            "agent's log, or not state-sensitive; plus runs with integrity anomalies."
        ),
    )
    triage.add_argument("--runs-root", default="runs", help="run root (default: runs)")
    triage.add_argument("--out", help="output directory (default: <runs-root>/_stats)")
    triage.add_argument("--backfill", action="store_true", help="collect missing metrics first")
    triage.add_argument("--limit", type=int, default=20, help="rows to print (default: 20)")
    triage.add_argument("--json", dest="print_json", action="store_true", help="print the triage JSON")
    triage.add_argument("--now", help="override generation timestamp (tests, backfills)")
    triage.add_argument("--quiet", action="store_true")

    report = subparsers.add_parser("report", help="aggregate metrics into a round report")
    report.add_argument("--runs-root", default="runs", help="run root (default: runs)")
    report.add_argument("--out", help="output directory (default: <runs-root>/_stats)")
    report.add_argument("--backfill", action="store_true", help="collect missing metrics first")
    report.add_argument("--expected", type=int, help="expected run count for round completeness")
    report.add_argument("--since", help="only runs started at/after this ISO-8601 timestamp")
    report.add_argument("--until", help="only runs started at/before this ISO-8601 timestamp")
    report.add_argument("--json", dest="print_json", action="store_true", help="print summary JSON")
    report.add_argument("--now", help="override generation timestamp (tests, backfills)")
    report.add_argument("--quiet", action="store_true")
    return parser


def _collect_one(run_dir: Path, force: bool, now: Optional[str]) -> Dict[str, Any]:
    target = metrics_path(run_dir)
    if target.exists() and not force:
        return {"run_id": run_dir.name, "status": "skipped", "path": str(target)}
    payload = collect_run(run_dir, now=now)
    path = write_run_metrics(run_dir, payload)
    return {
        "run_id": run_dir.name,
        "status": "written",
        "path": str(path),
        "findings": payload["report"]["findings_total"],
        "grade": payload["report"]["grade"],
        "anomalies": len(payload["anomalies"]),
    }


def command_collect(args: argparse.Namespace) -> int:
    if not args.run_dir and not args.runs_root:
        print("collect: provide RUN_DIR or --runs-root", file=sys.stderr)
        return 2

    targets: List[Path] = []
    if args.run_dir:
        run_dir = Path(args.run_dir)
        if not run_dir.is_dir():
            print("collect: not a directory: {}".format(run_dir), file=sys.stderr)
            return 2
        targets.append(run_dir)
    if args.runs_root:
        root = Path(args.runs_root)
        if not root.is_dir():
            print("collect: not a directory: {}".format(root), file=sys.stderr)
            return 2
        targets.extend(iter_run_dirs(root))

    results: List[Dict[str, Any]] = []
    failures = 0
    for run_dir in targets:
        try:
            result = _collect_one(run_dir, args.force, args.now)
        except Exception as error:  # a bad run must not stop a batch
            failures += 1
            results.append({"run_id": run_dir.name, "status": "error", "error": str(error)})
            if not args.quiet:
                print("collect: {} failed: {}".format(run_dir.name, error), file=sys.stderr)
            continue
        results.append(result)

    if args.print_json:
        if len(targets) == 1 and results and results[0].get("status") != "error":
            payload = collect_run(Path(targets[0]), now=args.now)
            print(json.dumps(payload, indent=2, ensure_ascii=False))
        else:
            print(json.dumps(results, indent=2, ensure_ascii=False))
        return 1 if failures else 0

    if not args.quiet:
        written = sum(1 for item in results if item["status"] == "written")
        skipped = sum(1 for item in results if item["status"] == "skipped")
        print(
            "collect: {} written, {} skipped, {} failed".format(written, skipped, failures),
            file=sys.stderr,
        )
        for item in results:
            if item["status"] == "written":
                print(
                    "  {run_id}: findings={findings} schema={grade} anomalies={anomalies}".format(**item),
                    file=sys.stderr,
                )
            elif item["status"] == "error":
                print("  {}: error={}".format(item["run_id"], item["error"]), file=sys.stderr)

    return 1 if failures else 0


def command_replay(args: argparse.Namespace) -> int:
    run_dir = Path(args.run_dir)
    if not run_dir.is_dir():
        print("replay: not a directory: {}".format(run_dir), file=sys.stderr)
        return 2

    payload = run_replay(
        run_dir,
        rpc_url=args.rpc_url,
        block=args.block,
        delta=args.delta,
        budget=args.budget_ms,
    )
    write_replay(run_dir, payload)

    if args.print_json:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
    elif not args.quiet:
        tests = payload.get("tests") or {}
        state_sensitive = sum(1 for row in tests.values() if row.get("state_sensitive") is True)
        print(
            "replay: status={} tests={} state_sensitive={} block={} delta_block={}".format(
                payload.get("status"),
                len(tests),
                state_sensitive,
                payload.get("block"),
                payload.get("delta_block"),
            ),
            file=sys.stderr,
        )
        if payload.get("reason"):
            print("  reason: {}".format(payload["reason"]), file=sys.stderr)
        for error in (payload.get("errors") or [])[:5]:
            print("  error: {}".format(error), file=sys.stderr)

    return 0 if payload.get("status") == "ok" else 1


def command_triage(args: argparse.Namespace) -> int:
    runs_root = Path(args.runs_root)
    if not runs_root.is_dir():
        print("triage: not a directory: {}".format(runs_root), file=sys.stderr)
        return 2

    loaded = load_metrics(runs_root, backfill=args.backfill, now=args.now)
    if not loaded["metrics"]:
        print("triage: no metrics found under {}".format(runs_root), file=sys.stderr)
        print("triage: run `audit_stats collect --runs-root {}` first".format(runs_root), file=sys.stderr)
        return 1

    payload = triage(
        loaded["metrics"],
        missing=loaded["missing"],
        errors=loaded["errors"],
        now=args.now,
    )
    out_dir = Path(args.out) if args.out else runs_root / "_stats"
    paths = write_triage(out_dir, payload)

    if args.print_json:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
    elif not args.quiet:
        for line in console_summary(payload, limit=args.limit):
            print(line)
        for key, path in paths.items():
            print("  {}: {}".format(key, path))
    return 0


def command_report(args: argparse.Namespace) -> int:
    runs_root = Path(args.runs_root)
    if not runs_root.is_dir():
        print("report: not a directory: {}".format(runs_root), file=sys.stderr)
        return 2

    out_dir = Path(args.out) if args.out else runs_root / "_stats"
    loaded = load_metrics(
        runs_root,
        since=args.since,
        until=args.until,
        backfill=args.backfill,
        now=args.now,
    )
    metrics = loaded["metrics"]

    if not metrics:
        print("report: no metrics found under {}".format(runs_root), file=sys.stderr)
        print("report: run `audit_stats collect --runs-root {}` first".format(runs_root), file=sys.stderr)
        return 1

    summary = aggregate(
        metrics,
        expected=args.expected,
        generated_at=args.now,
        runs_root=str(runs_root),
        missing=loaded["missing"],
        stale=loaded["stale"],
        errors=loaded["errors"],
    )
    paths = write_outputs(out_dir, summary, metrics)

    if args.print_json:
        print(json.dumps(summary, indent=2, ensure_ascii=False))
    elif not args.quiet:
        findings = summary["findings"]
        detection = summary["detection"]
        fork = summary["fork"]
        evidence = summary["evidence"]
        print("report: {} run(s), {} evaluable".format(summary["runs"]["total"], summary["runs"]["evaluable"]))
        print(
            "  findings={total} high+={high_plus} critical={critical} "
            "runs_high+={runs} ({rate})".format(
                total=findings["total"],
                high_plus=findings["high_plus"],
                critical=findings["critical"],
                runs=detection["runs_high_plus"],
                rate=format_pct(detection["rate_high_plus"]),
            )
        )
        print(
            "  fork_repro={passed}/{total} ({rate}); attested={attested} ({ar}); "
            "state_sensitive={state_sensitive}; evidence={resolved}/{refs} ({er})".format(
                passed=fork["repro_passed"],
                total=fork["findings"],
                rate=format_pct(fork["repro_rate"]),
                attested=fork["repro_attested"],
                ar=format_pct(fork["repro_attested_rate"]),
                state_sensitive=fork["repro_state_sensitive"],
                resolved=evidence["refs_resolved"],
                refs=evidence["refs_total"],
                er=format_pct(evidence["resolution_rate"]),
            )
        )
        function_coverage = summary["function_coverage"]
        stability = summary["stability"]
        print(
            "  function_coverage={covered}/{total} ({rate}, trace {trace})".format(
                covered=function_coverage["covered_total"],
                total=function_coverage["abi_functions_total"],
                rate=format_pct(function_coverage["coverage"]),
                trace=format_pct(function_coverage["trace_coverage"]),
            )
        )
        if stability["pairs"]:
            print(
                "  stability={pairs} pair(s), overlap {overlap}, |severity delta| {delta}".format(
                    pairs=stability["pairs"],
                    overlap=format_pct(stability["mean_overlap"]),
                    delta=(
                        "{:.2f}".format(stability["mean_abs_severity_delta"])
                        if stability["mean_abs_severity_delta"] is not None
                        else "n/a"
                    ),
                )
            )
        else:
            print("  stability=n/a (no target audited twice)")
        print(
            "  anomalies={total} in {runs} run(s); p50={p50} p90={p90}".format(
                total=summary["anomalies"]["total"],
                runs=summary["anomalies"]["runs_affected"],
                p50=format_seconds(summary["latency"]["p50"]),
                p90=format_seconds(summary["latency"]["p90"]),
            )
        )
        if summary["round"]["missing"]:
            print("  metrics missing for {} run(s)".format(summary["round"]["missing"]))
        if loaded["stale"]:
            print("  stale metrics schema for {} run(s)".format(len(loaded["stale"])))
        for key, path in paths.items():
            print("  {}: {}".format(key, path))

    return 0


def main(argv: Optional[List[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "collect":
        return command_collect(args)
    if args.command == "replay":
        return command_replay(args)
    if args.command == "triage":
        return command_triage(args)
    if args.command == "report":
        return command_report(args)
    parser.error("unknown command")
    return 2
