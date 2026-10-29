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
      "reproduction": "Exact command or test path, naming the test function.",
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

`reproduction` must name the exact `test*` function. After the run exits, that
test is re-run by the harness against `RPC_URL`; a finding whose test cannot be
re-run or fails on re-run is recorded as unattested.

Keep unsupported claims in `hypotheses`. Report missing RPC/source/tool access
and rate limits under `limitations`; do not present a blocked audit as complete.
