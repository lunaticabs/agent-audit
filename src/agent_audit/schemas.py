from __future__ import annotations

from dataclasses import dataclass


@dataclass
class ArtifactRecord:
    step: str
    path: str
    kind: str
    status: str
    summary: str
