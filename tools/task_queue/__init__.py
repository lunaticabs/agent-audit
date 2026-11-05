"""Redis Stream task queue used by the k3s runner in pull mode.

The runner no longer receives its prompt from a dispatcher Deployment. KEDA
creates one Job per queued task; the Job claims exactly one message from the
stream, runs the audit, and acknowledges the message only when the run
succeeds. A failed run leaves the message in this consumer's pending list so
the Job retry (``backoffLimit: 1``) can claim the same task again.
"""

__version__ = "0.1.0"
