"""Cross-run stability: title Jaccard overlap and severity drift.

Two audits of the same target are expected to agree on most findings. This
module matches findings across runs by normalized title similarity, then reports
how much the sets overlap and how far the severity moved. It measures
repeatability, not correctness: two runs can agree and both be wrong.
"""

from __future__ import annotations

import re
from typing import Any, Dict, List, Optional, Sequence, Tuple

from .common import percentile, ratio

DEFAULT_THRESHOLD = 0.6
DEFAULT_MAX_PAIRS = 2000

SEVERITY_RANK = {
    "critical": 4,
    "high": 3,
    "medium": 2,
    "low": 1,
    "informational": 0,
}

# Words that carry no matching signal. Kept deliberately small: aggressive
# stopword lists remove distinctions such as "cross-function" or "first".
STOPWORDS = {
    "a",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "been",
    "but",
    "by",
    "can",
    "could",
    "did",
    "do",
    "does",
    "for",
    "from",
    "has",
    "have",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "may",
    "might",
    "no",
    "not",
    "of",
    "on",
    "or",
    "should",
    "than",
    "that",
    "the",
    "their",
    "then",
    "there",
    "this",
    "to",
    "via",
    "was",
    "were",
    "when",
    "where",
    "which",
    "who",
    "will",
    "with",
    "would",
    "contract",
    "contracts",
    "function",
    "functions",
    "issue",
    "issues",
    "risk",
    "risks",
    "vulnerability",
    "vulnerabilities",
    "possible",
    "potential",
}

TOKEN_RE = re.compile(r"[a-z0-9]+")


def title_tokens(title: Any) -> set:
    if not isinstance(title, str):
        return set()
    tokens = {token for token in TOKEN_RE.findall(title.lower()) if len(token) > 2}
    return {token for token in tokens if token not in STOPWORDS}


def jaccard(left: set, right: set) -> float:
    if not left and not right:
        return 0.0
    union = left | right
    if not union:
        return 0.0
    return len(left & right) / len(union)


def _finding_rows(metrics: Dict[str, Any]) -> List[Dict[str, Any]]:
    rows = metrics.get("findings")
    if not isinstance(rows, list):
        return []
    return [row for row in rows if isinstance(row, dict)]


def match_findings(
    left: Sequence[Dict[str, Any]],
    right: Sequence[Dict[str, Any]],
    threshold: float = DEFAULT_THRESHOLD,
) -> List[Tuple[float, int, int]]:
    """Greedy one-to-one matching by title token Jaccard.

    Greedy is enough here: findings per run are few, and a perfect bipartite
    matching would cost more than it changes the overlap number.
    """
    left_tokens = [title_tokens(row.get("title")) for row in left]
    right_tokens = [title_tokens(row.get("title")) for row in right]

    candidates: List[Tuple[float, int, int]] = []
    for index, tokens in enumerate(left_tokens):
        if not tokens:
            continue
        for other, other_tokens in enumerate(right_tokens):
            if not other_tokens:
                continue
            score = jaccard(tokens, other_tokens)
            if score >= threshold:
                candidates.append((score, index, other))

    candidates.sort(reverse=True)
    used_left: set = set()
    used_right: set = set()
    matched: List[Tuple[float, int, int]] = []
    for score, index, other in candidates:
        if index in used_left or other in used_right:
            continue
        used_left.add(index)
        used_right.add(other)
        matched.append((score, index, other))
    return matched


def _severity_rank(value: Any) -> Optional[int]:
    if not isinstance(value, str):
        return None
    return SEVERITY_RANK.get(value)


def compare_pair(
    left: Dict[str, Any],
    right: Dict[str, Any],
    threshold: float = DEFAULT_THRESHOLD,
) -> Dict[str, Any]:
    """Compare two metrics records for the same target."""
    findings_left = _finding_rows(left)
    findings_right = _finding_rows(right)
    matched = match_findings(findings_left, findings_right, threshold)

    union = len(findings_left) + len(findings_right) - len(matched)
    empty_pair = union == 0
    overlap = 1.0 if empty_pair else (ratio(len(matched), union) or 0.0)

    deltas: List[int] = []
    scores: List[float] = []
    for score, index, other in matched:
        scores.append(score)
        left_rank = _severity_rank(findings_left[index].get("severity"))
        right_rank = _severity_rank(findings_right[other].get("severity"))
        if left_rank is not None and right_rank is not None:
            deltas.append(right_rank - left_rank)

    # Best-match score for every finding, with unmatched findings scored 0. This
    # is stricter than averaging matched pairs only.
    left_tokens = [title_tokens(row.get("title")) for row in findings_left]
    right_tokens = [title_tokens(row.get("title")) for row in findings_right]
    left_best = [
        max((jaccard(tokens, other) for other in right_tokens), default=0.0)
        for tokens in left_tokens
    ]
    right_best = [
        max((jaccard(tokens, other) for other in left_tokens), default=0.0)
        for tokens in right_tokens
    ]
    all_best = left_best + right_best

    run_left = left.get("run") or {}
    run_right = right.get("run") or {}
    target = left.get("target") or {}

    same_model = run_left.get("model") == run_right.get("model")
    same_image = run_left.get("image") == run_right.get("image")
    prompt_left = run_left.get("prompt_sha256")
    prompt_right = run_right.get("prompt_sha256")
    same_prompt: Optional[bool]
    if prompt_left is None and prompt_right is None:
        same_prompt = None
    else:
        same_prompt = prompt_left == prompt_right

    like_for_like = bool(same_model and same_image and same_prompt is not False)

    return {
        "chain": str(target.get("chain") or ""),
        "address": str(target.get("address") or "").lower(),
        "run_a": left.get("run_id"),
        "run_b": right.get("run_id"),
        "findings_a": len(findings_left),
        "findings_b": len(findings_right),
        "matched": len(matched),
        "union": union,
        "overlap": overlap,
        "empty_pair": empty_pair,
        "mean_best_jaccard": (sum(all_best) / len(all_best)) if all_best else None,
        "severity_delta_mean": (sum(deltas) / len(deltas)) if deltas else None,
        "severity_delta_max_abs": max((abs(delta) for delta in deltas), default=None),
        "severity_deltas": deltas,
        "max_severity_a": (left.get("report") or {}).get("max_severity"),
        "max_severity_b": (right.get("report") or {}).get("max_severity"),
        "only_in_a": len(findings_left) - len(matched),
        "only_in_b": len(findings_right) - len(matched),
        "same_model": same_model,
        "same_image": same_image,
        "same_prompt": same_prompt,
        "like_for_like": like_for_like,
    }


def _mean(values: Sequence[float]) -> Optional[float]:
    clean = [float(value) for value in values if isinstance(value, (int, float))]
    if not clean:
        return None
    return sum(clean) / len(clean)


def stability_summary(
    metrics: Sequence[Dict[str, Any]],
    threshold: float = DEFAULT_THRESHOLD,
    max_pairs: int = DEFAULT_MAX_PAIRS,
) -> Dict[str, Any]:
    """Group runs by target and compare every pair of repeated audits."""
    groups: Dict[Tuple[str, str], List[Dict[str, Any]]] = {}
    for item in metrics:
        target = item.get("target") or {}
        address = str(target.get("address") or "").strip().lower()
        if not address:
            continue
        chain = str(target.get("chain") or "").strip().lower()
        groups.setdefault((chain, address), []).append(item)

    repeated = {key: items for key, items in groups.items() if len(items) >= 2}

    pairs: List[Dict[str, Any]] = []
    truncated = False
    for key in sorted(repeated):
        items = sorted(
            repeated[key],
            key=lambda item: (
                str((item.get("run") or {}).get("started_at") or ""),
                str(item.get("run_id")),
            ),
        )
        for left_index in range(len(items)):
            for right_index in range(left_index + 1, len(items)):
                if len(pairs) >= max_pairs:
                    truncated = True
                    break
                pairs.append(compare_pair(items[left_index], items[right_index], threshold))
            if truncated:
                break
        if truncated:
            break

    overlaps = [pair["overlap"] for pair in pairs]
    abs_deltas = [
        abs(delta)
        for pair in pairs
        for delta in pair["severity_deltas"]
        if isinstance(delta, int)
    ]
    delta_distribution: Dict[str, int] = {}
    for pair in pairs:
        for delta in pair["severity_deltas"]:
            key = "{:+d}".format(delta)
            delta_distribution[key] = delta_distribution.get(key, 0) + 1

    like_for_like = [pair for pair in pairs if pair["like_for_like"]]
    changed = [pair for pair in pairs if not pair["like_for_like"]]

    worst: List[Dict[str, Any]] = []
    for key, items in repeated.items():
        target_pairs = [pair for pair in pairs if pair["address"] == key[1] and pair["chain"].lower() == key[0]]
        if not target_pairs:
            continue
        worst.append(
            {
                "chain": key[0],
                "address": key[1],
                "runs": len(items),
                "pairs": len(target_pairs),
                "mean_overlap": _mean([pair["overlap"] for pair in target_pairs]),
            }
        )
    worst.sort(key=lambda row: (row["mean_overlap"] if row["mean_overlap"] is not None else 1.0, row["address"]))
    worst = worst[:10]

    return {
        "threshold": threshold,
        "targets_with_repeat": len(repeated),
        "pairs": len(pairs),
        "truncated": truncated,
        "mean_overlap": _mean(overlaps),
        "median_overlap": percentile(overlaps, 0.5) if overlaps else None,
        "zero_overlap_pairs": sum(1 for value in overlaps if value == 0),
        "mean_abs_severity_delta": _mean(abs_deltas),
        "severe_drift_pairs": sum(
            1
            for pair in pairs
            if isinstance(pair.get("severity_delta_max_abs"), int) and pair["severity_delta_max_abs"] >= 2
        ),
        "severity_delta_distribution": dict(
            sorted(delta_distribution.items(), key=lambda item: int(item[0]))
        ),
        "like_for_like": {"pairs": len(like_for_like), "mean_overlap": _mean([p["overlap"] for p in like_for_like])},
        "changed_conditions": {"pairs": len(changed), "mean_overlap": _mean([p["overlap"] for p in changed])},
        "worst_targets": worst,
        "pairs_detail": pairs,
    }
