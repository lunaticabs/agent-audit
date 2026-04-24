#!/usr/bin/env python3

from __future__ import annotations

import glob
import os
import signal
import subprocess
import sys
import threading
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parent
DEFAULT_ADDRESS_DIR = ROOT / "addresses"
DEFAULT_LOG_DIR = ROOT / "batch_logs"

ACTIVE_PROCS: set[subprocess.Popen] = set()
ACTIVE_PROCS_LOCK = threading.Lock()
STOP_EVENT = threading.Event()


def register_proc(proc: subprocess.Popen) -> None:
    with ACTIVE_PROCS_LOCK:
        ACTIVE_PROCS.add(proc)


def unregister_proc(proc: subprocess.Popen) -> None:
    with ACTIVE_PROCS_LOCK:
        ACTIVE_PROCS.discard(proc)


def terminate_process(proc: subprocess.Popen, kill_grace_sec: int) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=kill_grace_sec)
    except Exception:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
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
    attach_url: str,
    model: str,
    agent: str,
    task_timeout_sec: int,
    kill_grace_sec: int,
) -> int:
    log_file = log_dir / f"{addr}.log"
    cmd = [
        "opencode",
        "run",
        "--attach",
        attach_url,
        "--agent",
        agent,
        "-m",
        model,
        f"Audit {addr} on eth.",
    ]

    if STOP_EVENT.is_set():
        return 130

    with log_file.open("w", encoding="utf-8") as out:
        out.write(f"[START] {addr}\n")
        out.flush()

        proc = subprocess.Popen(
            cmd,
            stdout=out,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
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

        out.write(f"[DONE ] {addr} exit={rc}\n")
    return rc


def list_session_ids() -> list[str]:
    proc = subprocess.run(
        ["opencode", "session", "list"],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        check=False,
    )
    ids: list[str] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if line.startswith("ses_"):
            ids.append(line.split()[0])
    return ids


def cleanup_sessions(keep_sessions: int) -> None:
    ids = list_session_ids()
    total = len(ids)
    if total <= keep_sessions:
        print(f"[CLEAN ] sessions={total} keep={keep_sessions} deleted=0")
        return

    delete_ids = ids[keep_sessions:]
    print(f"[CLEAN ] sessions={total} keep={keep_sessions} deleting={len(delete_ids)}")
    for sid in delete_ids:
        subprocess.run(
            ["opencode", "session", "delete", sid],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    print(f"[CLEAN ] deleted={len(delete_ids)}")


def bounded_submit(
    addresses: Iterable[str],
    max_jobs: int,
    log_dir: Path,
    attach_url: str,
    model: str,
    agent: str,
    enable_session_cleanup: bool,
    cleanup_every: int,
    keep_sessions: int,
    task_timeout_sec: int,
    kill_grace_sec: int,
) -> int:
    in_flight = set()
    addresses = list(addresses)
    total = len(addresses)
    started = 0
    completed = 0
    failed = 0
    lock = threading.Lock()

    with ThreadPoolExecutor(max_workers=max_jobs) as ex:
        for addr in addresses:
            if STOP_EVENT.is_set():
                break
            while len(in_flight) >= max_jobs:
                done, in_flight = wait(in_flight, return_when=FIRST_COMPLETED)
                for fut in done:
                    rc = fut.result()
                    completed += 1
                    if rc != 0:
                        failed += 1
                    print(f"[PROG ] done={completed}/{total} failed={failed}")

            started += 1
            print(f"[QUEUE] {started}/{total} {addr}")
            in_flight.add(
                ex.submit(
                    run_one,
                    addr,
                    log_dir,
                    attach_url,
                    model,
                    agent,
                    task_timeout_sec,
                    kill_grace_sec,
                )
            )

            if (
                enable_session_cleanup
                and cleanup_every > 0
                and started % cleanup_every == 0
            ):
                with lock:
                    cleanup_sessions(keep_sessions)

        while in_flight:
            if STOP_EVENT.is_set():
                terminate_all_processes(kill_grace_sec)
            done, in_flight = wait(in_flight, return_when=FIRST_COMPLETED)
            for fut in done:
                rc = fut.result()
                completed += 1
                if rc != 0:
                    failed += 1
                print(f"[PROG ] done={completed}/{total} failed={failed}")

    print(f"[FINAL] total={total} failed={failed}")
    return 0 if failed == 0 else 1


def main() -> int:
    max_jobs = env_int("MAX_JOBS", 8)
    cleanup_every = env_int("CLEANUP_EVERY", 50)
    keep_sessions = env_int("KEEP_SESSIONS", 200)
    task_timeout_sec = env_int("TASK_TIMEOUT_SEC", 120)
    kill_grace_sec = env_int("KILL_GRACE_SEC", 5)
    enable_session_cleanup = os.getenv("ENABLE_SESSION_CLEANUP", "1") == "1"

    address_dir = Path(os.getenv("ADDRESS_DIR", str(DEFAULT_ADDRESS_DIR))).resolve()
    address_file = os.getenv("ADDRESS_FILE")
    log_dir = Path(os.getenv("LOG_DIR", str(DEFAULT_LOG_DIR))).resolve()
    attach_url = os.getenv("OPENCODE_ATTACH", "http://127.0.0.1:4096")
    model = os.getenv("MODEL", "apiapi/gpt-5.3-codex")
    agent = os.getenv("AGENT", "audit")

    if max_jobs <= 0:
        print("[ERR  ] MAX_JOBS must be > 0", file=sys.stderr)
        return 2
    if keep_sessions < 0:
        print("[ERR  ] KEEP_SESSIONS must be >= 0", file=sys.stderr)
        return 2
    if task_timeout_sec <= 0:
        print("[ERR  ] TASK_TIMEOUT_SEC must be > 0", file=sys.stderr)
        return 2
    if kill_grace_sec < 0:
        print("[ERR  ] KILL_GRACE_SEC must be >= 0", file=sys.stderr)
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
            attach_url=attach_url,
            model=model,
            agent=agent,
            enable_session_cleanup=enable_session_cleanup,
            cleanup_every=cleanup_every,
            keep_sessions=keep_sessions,
            task_timeout_sec=task_timeout_sec,
            kill_grace_sec=kill_grace_sec,
        )
    finally:
        terminate_all_processes(kill_grace_sec)


if __name__ == "__main__":
    raise SystemExit(main())
