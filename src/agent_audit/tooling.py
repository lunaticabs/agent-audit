from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Sequence


@dataclass
class ToolExecutionResult:
    command: List[str]
    cwd: str
    returncode: int
    stdout: str
    stderr: str
    duration_ms: int


def has_flake_toolchain(project_root: Path) -> bool:
    return (project_root / "flake.nix").exists() and shutil.which("nix") is not None


def tool_available(tool_name: str, *, project_root: Path) -> bool:
    if shutil.which(tool_name):
        return True
    if has_flake_toolchain(project_root):
        return True
    return False


def prefixed_command(
    *, project_root: Path, base_command: Sequence[str], force_nix: bool = False
) -> List[str]:
    command = list(base_command)
    if force_nix or (has_flake_toolchain(project_root) and shutil.which(command[0]) is None):
        return ["nix", "develop", ".#default", "-c", *command]
    return command


def run_command(
    *,
    project_root: Path,
    base_command: Sequence[str],
    cwd: Path,
    env: Optional[Dict[str, str]] = None,
    timeout: int = 300,
    force_nix: bool = False,
) -> ToolExecutionResult:
    command = prefixed_command(
        project_root=project_root,
        base_command=base_command,
        force_nix=force_nix,
    )
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)

    started = time.perf_counter()
    completed = subprocess.run(
        command,
        cwd=str(cwd),
        env=merged_env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    duration_ms = int((time.perf_counter() - started) * 1000)
    return ToolExecutionResult(
        command=command,
        cwd=str(cwd),
        returncode=completed.returncode,
        stdout=completed.stdout,
        stderr=completed.stderr,
        duration_ms=duration_ms,
    )


def split_command(command: str) -> List[str]:
    return shlex.split(command)
