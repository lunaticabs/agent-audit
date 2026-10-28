# Per-run MongoDB persistence

`run_persist` mirrors exactly one finished audit run into MongoDB:

- `runs_meta` — one document per run. `target_address` and `target_chain` are
  top-level fields; the full `stats/metrics.json` object is embedded under
  `stats`; `runs_files` counts and timestamps stay top-level.
- `runs_files` — one document per archived workspace file. Only the run's own
  `input/`, `reports/`, `artifacts/`, `sources/`, `harness/`, and `test/`
  directories (plus a root `foundry.toml`) are read. Batch-level directories
  (`runs/_stats`, `runs/k3s-batch-*`, `batch_logs/`) are never walked because
  the entrypoint passes a single run directory.

The entrypoint calls this once, after `stats/metrics.json` has been collected
and before the optional `AGENT_AUDIT_PERSIST_DIR` copy. The database write is
best-effort: a failure is written to `runs/<run_id>/logs/persist.log` and does
not change the audit exit code.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_AUDIT_MONGO_URI` | unset | MongoDB URI. Persistence is skipped when unset. |
| `AGENT_AUDIT_MONGO_DB` | `agent` | Database name. |
| `AGENT_AUDIT_MONGO_RUNS_META_COLLECTION` | `runs_meta` | Metadata collection. |
| `AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION` | `runs_files` | File collection. |
| `AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES` | `8388608` | Larger files are skipped, not fatal. |
| `AGENT_AUDIT_MONGO_TIMEOUT_MS` | `5000` | Server-selection/connect timeout. |
| `AGENT_AUDIT_MONGO_RETRIES` | `3` | Attempts for a transient failure. |
| `AGENT_AUDIT_MONGO_CHUNK_SIZE` | `500` | File upserts per `bulk_write`. |
| `AGENT_AUDIT_MONGO_PERSIST` | `1` | Set to `0` to disable persistence. |

## Commands

```bash
# create the indexes used by new documents (safe to re-run; existing indexes
# are not dropped)
PYTHONPATH=tools .venv/bin/python -m run_persist --init-db

# inspect the document/files without connecting to MongoDB
PYTHONPATH=tools .venv/bin/python -m run_persist --dry-run runs/<run_id>

# persist one run manually
PYTHONPATH=tools .venv/bin/python -m run_persist runs/<run_id>
```

The package needs `pymongo`; the runner image installs
[requirements.txt](requirements.txt). The unit tests use fake collections and
do not need a database:

```bash
.venv/bin/python -m unittest discover -s tools/run_persist/tests -t tools -v
```

## Schema

```json
{
  "_id": "<run_id>",
  "schema_version": 2,
  "status": "succeeded",
  "created_at": "2026-10-28T01:00:00Z",
  "finished_at": "2026-10-28T01:10:00Z",
  "duration_seconds": 600.0,
  "target_address": "0x...",
  "target_chain": "eth",
  "block_number": "26000000",
  "has_final_report": true,
  "file_count": 41,
  "total_size_bytes": 332222,
  "stats": { "...": "verbatim stats/metrics.json" }
}
```

Historical `runs_meta` documents are intentionally not migrated. They keep the
old nested `target.address` / `target.chain` shape and have no `stats` field.
