# agent-audit-dispatcher (retired)

This Rust service consumed the Redis Stream, created one Kubernetes Job per
message, and injected `FULL_PROMPT` / `TASK_ID` / `RUN_ID` as environment
variables.

It was replaced by a **KEDA ScaledJob plus runner-side claim**:

- KEDA watches consumer-group lag and decides how many Jobs may exist.
- The runner pod claims one message itself (`tools/task_queue`), uses the Job
  name as its consumer name so a retry reclaims the same task, and acknowledges
  only after a successful run.

See [../k3s/README.md](../k3s/README.md) for the deployed topology.

The crate is kept because it still builds and documents the push model, and it
is the rollback path if KEDA has to be removed. It is no longer deployed, and
`k3s/` no longer contains its manifests. Do **not** run it against a stream that
KEDA also watches: both would create Jobs for the same task.
