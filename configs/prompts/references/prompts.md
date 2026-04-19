# Prompt Examples

These are examples for humans. They are not runtime defaults.

## Full Preparation

```text
Check AGENTS.md and inspect 0xB2185950F5A0A46687ac331916508aadA202e063 on eth.
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

Audit 0x4cD00E387622C35bDDB9b4c962C136462338BC31 on eth.

note:

Slither is not the verdict. Slither is only a hint source.
Do not produce a final report that mainly restates Slither warnings.

You must actively try Foundry, cast, anvil, and Echidna where relevant.
If a serious hypothesis exists, try to reproduce it or disprove it before finalizing.

Do not conclude any thing is secure unless you have specifically examined and validated it.