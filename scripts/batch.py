#!/usr/bin/env python3

from __future__ import annotations

import glob
import os
import signal
import shutil
import subprocess
import sys
import threading
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parent
PROJECT_ROOT = ROOT.parent
DEFAULT_ADDRESS_DIR = ROOT / "addresses"
DEFAULT_LOG_DIR = ROOT / "batch_logs"

ACTIVE_PROCS: set[subprocess.Popen] = set()
ACTIVE_PROCS_LOCK = threading.Lock()
STOP_EVENT = threading.Event()
VALID_SANDBOX_MODES = {"read-only", "workspace-write", "danger-full-access"}


def register_proc(proc: subprocess.Popen) -> None:
    with ACTIVE_PROCS_LOCK:
        ACTIVE_PROCS.add(proc)


def unregister_proc(proc: subprocess.Popen) -> None:
    with ACTIVE_PROCS_LOCK:
        ACTIVE_PROCS.discard(proc)


def terminate_process(proc: subprocess.Popen, kill_grace_sec: int) -> None:
    if proc.poll() is not None:
        return

    pgid: int | None = None
    try:
        pgid = os.getpgid(proc.pid)
    except Exception:
        pgid = None

    def _signal_term() -> None:
        if pgid is not None:
            try:
                os.killpg(pgid, signal.SIGTERM)
            except Exception:
                pass
        try:
            proc.terminate()
        except Exception:
            pass

    def _signal_kill() -> None:
        if pgid is not None:
            try:
                os.killpg(pgid, signal.SIGKILL)
            except Exception:
                pass
        try:
            proc.kill()
        except Exception:
            pass

    def _kill_direct_children(sig: str) -> None:
        if proc.poll() is not None:
            return
        pkill_cmd = ["pkill", f"-{sig}", "-P", str(proc.pid)]
        subprocess.run(
            pkill_cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )

    try:
        _signal_term()
        _kill_direct_children("TERM")
        proc.wait(timeout=kill_grace_sec)
        return
    except Exception:
        try:
            _signal_kill()
            _kill_direct_children("KILL")
            proc.wait(timeout=max(1, kill_grace_sec))
        except Exception:
            pass


def terminate_all_processes(kill_grace_sec: int) -> None:
    with ACTIVE_PROCS_LOCK:
        procs = list(ACTIVE_PROCS)
    for proc in procs:
        terminate_process(proc, kill_grace_sec)


def env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or raw.strip() == "":
        return default
    try:
        value = int(raw)
    except ValueError:
        print(f"[WARN ] invalid {name}={raw!r}, using {default}")
        return default
    return value


def load_addresses(address_dir: Path, address_file: str | None) -> list[str]:
    files: list[Path]
    if address_file:
        file_path = Path(address_file)
        if not file_path.is_absolute():
            file_path = address_dir / file_path
        files = [file_path]
    else:
        files = [Path(p) for p in sorted(glob.glob(str(address_dir / "*.txt")))]

    if not files:
        raise FileNotFoundError(f"no address file found in {address_dir}")

    addresses: list[str] = []
    seen: set[str] = set()
    loaded = 0

    for file_path in files:
        if not file_path.exists():
            raise FileNotFoundError(f"address file not found: {file_path}")

        with file_path.open("r", encoding="utf-8") as f:
            for line in f:
                text = line.strip()
                if not text or text.startswith("#"):
                    continue
                if text in seen:
                    continue
                seen.add(text)
                addresses.append(text)
                loaded += 1

    if not addresses:
        raise RuntimeError("no valid addresses loaded")

    print(f"[LOAD ] files={len(files)} addresses={loaded} dir={address_dir}")
    return addresses


def run_one(
    addr: str,
    log_dir: Path,
    model: str | None,
    sandbox: str | None,
    prompt_template: str,
    task_timeout_sec: int,
    kill_grace_sec: int,
) -> int:
    log_file = log_dir / f"{addr}.log"
    cmd = [
        "codex",
        "exec",
        "--ephemeral",
        "--color",
        "never",
    ]
    if sandbox:
        cmd.extend(["-s", sandbox])
    if model:
        cmd.extend(["-m", model])
    cmd.append(prompt_template.format(address=addr))

    with log_file.open("w", encoding="utf-8", buffering=1) as out:
        out.write(f"[START] {addr}\n")
        out.flush()

        if STOP_EVENT.is_set():
            out.write("[SKIP ] stop event already set\n")
            out.write(f"[DONE ] {addr} exit=130\n")
            out.flush()
            return 130

        try:
            proc = subprocess.Popen(
                cmd,
                stdout=out,
                stderr=subprocess.STDOUT,
                text=True,
                start_new_session=True,
                cwd=PROJECT_ROOT,
            )
            register_proc(proc)

            rc: int
            try:
                rc = proc.wait(timeout=task_timeout_sec)
            except subprocess.TimeoutExpired:
                out.write(f"[TIMEO] {addr} timeout={task_timeout_sec}s\n")
                out.flush()
                terminate_process(proc, kill_grace_sec)
                rc = 124
            finally:
                unregister_proc(proc)
        except Exception as exc:
            out.write(f"[ERROR] launch_failed {type(exc).__name__}: {exc}\n")
            out.flush()
            rc = 125

        out.write(f"[DONE ] {addr} exit={rc}\n")
        out.flush()
    return rc


def bounded_submit(
    addresses: Iterable[str],
    max_jobs: int,
    log_dir: Path,
    model: str | None,
    sandbox: str | None,
    prompt_template: str,
    task_timeout_sec: int,
    kill_grace_sec: int,
) -> int:
    in_flight: dict = {}
    addresses = list(addresses)
    total = len(addresses)
    started = 0
    completed = 0
    failed = 0
    timed_out = 0
    cancelled = 0

    with ThreadPoolExecutor(max_workers=max_jobs) as ex:
        for addr in addresses:
            if STOP_EVENT.is_set():
                break
            while len(in_flight) >= max_jobs:
                done, _ = wait(in_flight.keys(), return_when=FIRST_COMPLETED)
                for fut in done:
                    done_addr = in_flight.pop(fut)
                    try:
                        rc = fut.result()
                    except Exception as exc:
                        rc = 125
                        print(
                            f"[FAIL ] addr={done_addr} rc=125 error={type(exc).__name__}:{exc}"
                        )
                    completed += 1
                    if rc == 130:
                        cancelled += 1
                    elif rc != 0:
                        failed += 1
                        if rc == 124:
                            timed_out += 1
                        print(
                            f"[FAIL ] addr={done_addr} rc={rc} log={log_dir / f'{done_addr}.log'}"
                        )
                    print(
                        f"[PROG ] done={completed}/{total} failed={failed} "
                        f"timeout={timed_out} cancelled={cancelled}"
                    )

            started += 1
            print(f"[QUEUE] {started}/{total} {addr}")
            fut = ex.submit(
                run_one,
                addr,
                log_dir,
                model,
                sandbox,
                prompt_template,
                task_timeout_sec,
                kill_grace_sec,
            )
            in_flight[fut] = addr

        while in_flight:
            if STOP_EVENT.is_set():
                terminate_all_processes(kill_grace_sec)
            done, _ = wait(in_flight.keys(), return_when=FIRST_COMPLETED)
            for fut in done:
                done_addr = in_flight.pop(fut)
                try:
                    rc = fut.result()
                except Exception as exc:
                    rc = 125
                    print(
                        f"[FAIL ] addr={done_addr} rc=125 error={type(exc).__name__}:{exc}"
                    )
                completed += 1
                if rc == 130:
                    cancelled += 1
                elif rc != 0:
                    failed += 1
                    if rc == 124:
                        timed_out += 1
                    print(
                        f"[FAIL ] addr={done_addr} rc={rc} log={log_dir / f'{done_addr}.log'}"
                    )
                print(
                    f"[PROG ] done={completed}/{total} failed={failed} "
                    f"timeout={timed_out} cancelled={cancelled}"
                )

    print(
        f"[FINAL] total={total} failed={failed} timeout={timed_out} cancelled={cancelled}"
    )
    return 0 if failed == 0 else 1


def main() -> int:
    max_jobs = env_int("MAX_JOBS", 8)
    task_timeout_sec = env_int("TASK_TIMEOUT_SEC", 900)
    kill_grace_sec = env_int("KILL_GRACE_SEC", 5)

    address_dir = Path(os.getenv("ADDRESS_DIR", str(DEFAULT_ADDRESS_DIR))).resolve()
    address_file = os.getenv("ADDRESS_FILE")
    log_dir = Path(os.getenv("LOG_DIR", str(DEFAULT_LOG_DIR))).resolve()
    model = os.getenv("MODEL")
    sandbox = os.getenv("CODEX_SANDBOX")
    prompt_template = os.getenv("PROMPT_TEMPLATE", "Check AGENTS.md and Audit {address} on eth.")

    print(
        "[CONF ] "
        f"max_jobs={max_jobs} "
        f"timeout={task_timeout_sec}s "
        f"log_dir={log_dir} "
        f"address_dir={address_dir} "
        f"project_root={PROJECT_ROOT} "
        f"sandbox={sandbox or '<config>'} "
        f"model={model or '<config>'}"
    )

    if max_jobs <= 0:
        print("[ERR  ] MAX_JOBS must be > 0", file=sys.stderr)
        return 2
    if task_timeout_sec <= 0:
        print("[ERR  ] TASK_TIMEOUT_SEC must be > 0", file=sys.stderr)
        return 2
    if kill_grace_sec < 0:
        print("[ERR  ] KILL_GRACE_SEC must be >= 0", file=sys.stderr)
        return 2
    if sandbox is not None and sandbox.lower() not in VALID_SANDBOX_MODES:
        print(f"[ERR  ] unsupported CODEX_SANDBOX={sandbox!r}", file=sys.stderr)
        return 2
    if "{address}" not in prompt_template:
        print("[ERR  ] PROMPT_TEMPLATE must contain {address}", file=sys.stderr)
        return 2
    if shutil.which("codex") is None:
        print("[ERR  ] codex command not found in PATH", file=sys.stderr)
        return 2

    def _handle_signal(signum: int, _frame: object) -> None:
        STOP_EVENT.set()
        print(f"[SIGNL] received={signum}, stopping and terminating children")
        terminate_all_processes(kill_grace_sec)

    signal.signal(signal.SIGINT, _handle_signal)
    signal.signal(signal.SIGTERM, _handle_signal)

    log_dir.mkdir(parents=True, exist_ok=True)

    try:
        addresses = load_addresses(address_dir, address_file)
    except Exception as exc:
        print(f"[ERR  ] {exc}", file=sys.stderr)
        return 2

    try:
        return bounded_submit(
            addresses=addresses,
            max_jobs=max_jobs,
            log_dir=log_dir,
            model=model,
            sandbox=sandbox,
            prompt_template=prompt_template,
            task_timeout_sec=task_timeout_sec,
            kill_grace_sec=kill_grace_sec,
        )
    finally:
        terminate_all_processes(kill_grace_sec)


if __name__ == "__main__":
    raise SystemExit(main())
