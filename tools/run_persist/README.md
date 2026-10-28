# Per-run MongoDB persistence

`run_persist` mirrors exactly one finished audit run into MongoDB:

- `runs_meta` — one document per run. `target_address` and `target_chain` are
  top-level fields; the full `stats/metrics.json` object is embedded under
  `stats`; `runs_files` counts and timestamps stay top-level.
- `runs_files` — one document for every regular file under that run directory,
  including logs, stats, build output, and root files. `.run.lock`,
  `logs/persist.log`, and symlinks escaping the run are skipped; files over the
  inline limit are recorded in
  `skipped_files`. Batch-level directories (`runs/_stats`, `runs/k3s-batch-*`,
  `batch_logs/`) are never walked because the entrypoint passes a single run
  directory.
- `runs_meta.materials` — one manifest entry per archived file with
  `rel_path`, `kind`, `size_bytes`, and `sha256`. This is the evidence index
  used to resolve paths cited by the report.

`block_number` is resolved from `input/run_meta.json`, the report, or
`artifacts/pinned_block.json`. If none has one, `run_persist` calls
`eth_blockNumber` on `AGENT_AUDIT_RPC_URL` / `RPC_URL` at archive time and
records `block_number_source`.

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
| `AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES` | `8388608` | Larger files are listed in `skipped_files`, not fatal. |
| `AGENT_AUDIT_MONGO_TIMEOUT_MS` | `5000` | Server-selection/connect timeout. |
| `AGENT_AUDIT_MONGO_RETRIES` | `3` | Attempts for a transient failure. |
| `AGENT_AUDIT_RPC_URL` / `RPC_URL` | unset | Used for `eth_blockNumber` when the run saved no block. |
| `AGENT_AUDIT_RPC_TIMEOUT_MS` | `5000` | Timeout for that RPC call. |
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
  "block_number_source": "pinned_block",
  "has_final_report": true,
  "file_count": 188,
  "total_size_bytes": 4026773,
  "materials": [
    {"rel_path": "artifacts/slither.json", "kind": "json", "size_bytes": 25067, "sha256": "..."}
  ],
  "stats": { "...": "verbatim stats/metrics.json" }
}
```

Historical `runs_meta` documents are intentionally not migrated. They keep the
old nested `target.address` / `target.chain` shape and have no `stats` field.
