# Prompt Examples

These are examples for humans. They are not runtime defaults.

## Full Preparation

```text
Use $smart-contract-audit to inspect 0x... on eth.
Create a run, prepare source, chain, IR, dependency, and static-analysis materials,
then read reports/materials_manifest.json before deciding whether deeper validation is needed.
```

## Static-First Triage

```text
Use $smart-contract-audit to inspect 0x... on eth.
Focus on preparing source, IR, dependency, and Slither materials only.
Do not jump to final conclusions before reading the raw artifacts.
```

## Hypothesis-Driven Validation

```text
Use $smart-contract-audit to inspect 0x... on eth.
Prepare the normal materials first. If a specific high-signal hypothesis looks worth validating,
decide whether to invoke Echidna and explain why that hypothesis merits fuzzing.
```
