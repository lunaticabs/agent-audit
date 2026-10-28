# Report

Write `reports/final_report.json` before finishing.

```json
{
  "run_id": "<RUN_ID>",
  "target": {
    "chain": "<alias or id>",
    "address": "0x...",
    "name": "<contract name if known>",
    "verified": true
  },
  "status": "completed | incomplete | blocked",
  "summary": "Evidence-based summary.",
  "findings": [
    {
      "id": "F-1",
      "title": "...",
      "severity": "critical | high | medium | low | informational",
      "confidence": "confirmed | likely",
      "contract": "0x...",
      "function": "...",
      "impact": "...",
      "evidence": ["artifacts/...", "sources/..."],
      "reproduction": "Exact command or test path.",
      "recommendation": "..."
    }
  ],
  "hypotheses": [
    { "title": "...", "reason": "...", "evidence": ["artifacts/..."] }
  ],
  "limitations": ["..."],
  "coverage": {
    "source_review": true,
    "static_analysis": false,
    "fork_testing": true,
    "fuzzing": false
  }
}
```

A confirmed finding needs evidence another reviewer can open: source reference,
tool output, trace, fork-test result with harness source, minimized calldata, or
a measured balance/accounting delta.

Keep unsupported claims in `hypotheses`. Report missing RPC/source/tool access
and rate limits under `limitations`; do not present a blocked audit as complete.
