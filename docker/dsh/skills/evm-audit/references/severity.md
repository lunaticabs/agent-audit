# Severity

`severity` is a triage label, not a loss estimate. Keep the report's five-level
vocabulary (`critical | high | medium | low | informational`); score two axes
first, then map.

## Six questions

Answer all six before writing a finding's `severity`.

1. **Impact** — what is the worst realistic outcome?
   `principal theft / permanent freeze` > `temporary freeze` > `yield theft` >
   `griefing / DoS / accounting` > `best practice / no loss`.
2. **Who can trigger it?** — `any address` > `single EOA/key` > `k-of-n Safe` >
   `timelock / governance` > `user must make a mistake`.
3. **Preconditions** — oracle or market move, timing, external integration,
   specific state, repeatability. Would a competent attacker need luck?
4. **Trust model** — is the role trusted by design? Owner/admin/executor powers
   are assumed trustworthy unless the protocol claims to be trust-minimized.
   "Owner can drain" is a centralization finding, not automatically a code bug.
5. **Scope and proof** — is it in scope, reachable now, and backed by a PoC or
   artifact? Reject hand-wavy hypotheticals, future code, OOS dependencies,
   weird tokens, and user mistakes.
6. **If unfixed** — protocol insolvency / full loss, degraded function, or
   inconvenience? Use this as the tie-breaker.

## Baseline mapping

| Impact \ Likelihood | High: anyone, no preconditions | Medium: single key, or real preconditions | Low: multisig/timelock/governance, or user error |
| --- | --- | --- | --- |
| Principal loss / permanent freeze | critical | high | medium |
| Temporary freeze / yield theft | high | medium | low |
| Griefing / DoS / accounting | medium | low | informational |
| Best practice / no loss | low | informational | informational |

Adjust one level with the six answers; explain the adjustment in `impact`.

- **Critical** needs a reachable path to principal loss or permanent freeze.
  A large TVL alone is not enough.
- **High** needs direct loss, or a valid indirect path with stated assumptions.
  If the only path is an admin action, cap at medium.
- **Medium** covers assets not at direct risk, degraded availability, or
  privileged-function scenarios with reasonable assumptions.
- **Low / informational** covers best practice, dust, user error, approve race,
  zero-address transfers, and view-only issues.

## Guardrails

- **Do not use TVL or the dollar amount at risk as the severity.** Record the
  amount in `impact`, but keep it out of the level decision.
- **"Any address can call" is a likelihood factor, not an impact.** It does not
  make a no-loss issue critical.
- **Latent paths** — needs an admin upgrade, a future integration, or a token
  that does not exist yet — have lower likelihood; state the condition.
- **Privileged-role-only findings** (owner, 3/5, 4/6, executor) are at most
  medium unless the protocol's threat model treats that role as adversarial.
- **One finding, one severity.** If there are multiple paths, score the easiest
  reachable path with material impact and mention the rest.
- **Write the rationale.** End `impact` with:
  `Severity basis: impact=<...>; likelihood=<...>; trust=<...>; scope=<...>.`
- **When genuinely ambiguous, choose the lower level** and state the assumption.
  Do not inflate a finding to get attention.
