"""Post-run statistics for agent-audit run directories.

The run directory is the durable artifact. This package derives a small,
versioned metrics record from one run (`collect`) and aggregates many records
(`report`). It never mutates audit evidence and it never trusts the agent's
self-reported fields: every claim that can be checked against a file on disk is
checked.

Layout produced by `collect`:

    runs/<run_id>/stats/metrics.json

`stats/` is derived data. It can be deleted and regenerated at any time.
"""

from __future__ import annotations

__version__ = "0.1.0"

STATS_SCHEMA_VERSION = 2

__all__ = ["__version__", "STATS_SCHEMA_VERSION"]
