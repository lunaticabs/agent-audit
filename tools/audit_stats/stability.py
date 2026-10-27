"""Cross-run stability: title Jaccard overlap and severity drift.

Two audits of the same target are expected to agree on most findings. This
module matches findings across runs by normalized title similarity, then reports
how much the sets overlap and how far the severity moved. It measures
repeatability, not correctness: two runs can agree and both be wrong.
"""

from __future__ import annotations

import difflib
import re
from typing import Any, Dict, List, Optional, Sequence, Tuple

from .common import percentile, ratio

DEFAULT_THRESHOLD = 0.5
FUNCTION_FLOOR = 0.35
FUNCTION_BONUS = 0.15
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
FUNCTION_KEY_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(")


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


def overlap_coefficient(left: set, right: set) -> float:
    """Intersection over the smaller set; forgiving when one title adds words."""
    if not left or not right:
        return 0.0
    return len(left & right) / min(len(left), len(right))


def normalized_title(text: Any) -> str:
    if not isinstance(text, str):
        return ""
    return " ".join(TOKEN_RE.findall(text.lower()))


def title_similarity(left: Any, right: Any) -> Tuple[float, float, float, float]:
    """Return (best, jaccard, overlap, sequence-ratio).

    Titles are model-authored, so lexical overlap alone is too brittle. The
    maximum of three signals is the best cheap proxy; the function field is a
    separate anchor in `match_findings`.
    """
    left_tokens = title_tokens(left)
    right_tokens = title_tokens(right)
    jac = jaccard(left_tokens, right_tokens)
    overlap = overlap_coefficient(left_tokens, right_tokens)
    sequence = difflib.SequenceMatcher(
        None, normalized_title(left), normalized_title(right)
    ).ratio()
    return max(jac, overlap, sequence), jac, overlap, sequence


def function_keys(value: Any) -> set:
    if not isinstance(value, str):
        return set()
    return {match.group(1).lower() for match in FUNCTION_KEY_RE.finditer(value)}


def _finding_rows(metrics: Dict[str, Any]) -> List[Dict[str, Any]]:
    rows = metrics.get("findings")
    if not isinstance(rows, list):
        return []
    return [row for row in rows if isinstance(row, dict)]


def match_findings(
    left: Sequence[Dict[str, Any]],
    right: Sequence[Dict[str, Any]],
    threshold: float = DEFAULT_THRESHOLD,
) -> List[Tuple[float, int, int, bool]]:
    """Greedy one-to-one matching, anchored by `function` when available.

    A pair needs either:
      - overlapping function keys (e.g. both `withdraw(uint256)`) plus a title
        similarity of at least `FUNCTION_FLOOR`, or
      - a title similarity of at least `threshold` when functions are absent or
        disjoint.

    The function anchor matters: in a real repeated audit the same four issues
    were reworded heavily and token Jaccard was only 0.17-0.33, while the
    `function` field still matched. Greedy is enough for the small number of
    findings per run; a perfect bipartite matching would not change the number.
    """
    left_tokens = [title_tokens(row.get("title")) for row in left]
    right_tokens = [title_tokens(row.get("title")) for row in right]
    left_norm = [normalized_title(row.get("title")) for row in left]
    right_norm = [normalized_title(row.get("title")) for row in right]
    left_funcs = [function_keys(row.get("function")) for row in left]
    right_funcs = [function_keys(row.get("function")) for row in right]

    candidates: List[Tuple[float, float, bool, int, int]] = []
    for index in range(len(left)):
        if not left_tokens[index] and not left_funcs[index]:
            continue
        for other in range(len(right)):
            if not right_tokens[other] and not right_funcs[other]:
                continue
            jac = jaccard(left_tokens[index], right_tokens[other])
            overlap = overlap_coefficient(left_tokens[index], right_tokens[other])
            sequence = difflib.SequenceMatcher(
                None, left_norm[index], right_norm[other]
            ).ratio()
            similarity = max(jac, overlap, sequence)
            anchored = bool(left_funcs[index] & right_funcs[other])

            if anchored:
                if similarity < FUNCTION_FLOOR:
                    continue
            elif similarity < threshold:
                continue

            score = similarity + (FUNCTION_BONUS if anchored else 0.0)
            candidates.append((score, similarity, anchored, index, other))

    candidates.sort(reverse=True)
    used_left: set = set()
    used_right: set = set()
    matched: List[Tuple[float, int, int, bool]] = []
    for _, similarity, anchored, index, other in candidates:
        if index in used_left or other in used_right:
            continue
        used_left.add(index)
        used_right.add(other)
        matched.append((similarity, index, other, anchored))
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
    matched_by_function = 0
    for similarity, index, other, anchored in matched:
        scores.append(similarity)
        if anchored:
            matched_by_function += 1
        left_rank = _severity_rank(findings_left[index].get("severity"))
        right_rank = _severity_rank(findings_right[other].get("severity"))
        if left_rank is not None and right_rank is not None:
            deltas.append(right_rank - left_rank)

    # Best-match score for every finding, with unmatched findings scored 0.
    # Both the combined similarity (used for matching) and pure token Jaccard
    # (a diagnostic) are reported; they diverge sharply on reworded titles.
    left_best_similarity: List[float] = []
    right_best_similarity: List[float] = []
    left_best_jaccard: List[float] = []
    right_best_jaccard: List[float] = []
    for row in findings_left:
        left_best_similarity.append(
            max(
                (title_similarity(row.get("title"), other.get("title"))[0] for other in findings_right),
                default=0.0,
            )
        )
        left_best_jaccard.append(
            max(
                (jaccard(title_tokens(row.get("title")), title_tokens(other.get("title"))) for other in findings_right),
                default=0.0,
            )
        )
    for row in findings_right:
        right_best_similarity.append(
            max(
                (title_similarity(row.get("title"), other.get("title"))[0] for other in findings_left),
                default=0.0,
            )
        )
        right_best_jaccard.append(
            max(
                (jaccard(title_tokens(row.get("title")), title_tokens(other.get("title"))) for other in findings_left),
                default=0.0,
            )
        )
    all_best = left_best_similarity + right_best_similarity
    all_best_jaccard = left_best_jaccard + right_best_jaccard

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
        "matched_by_function": matched_by_function,
        "matched_by_title": len(matched) - matched_by_function,
        "mean_best_similarity": (sum(all_best) / len(all_best)) if all_best else None,
        "mean_best_jaccard": (sum(all_best_jaccard) / len(all_best_jaccard)) if all_best_jaccard else None,
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
        "function_floor": FUNCTION_FLOOR,
        "targets_with_repeat": len(repeated),
        "pairs": len(pairs),
        "truncated": truncated,
        "mean_overlap": _mean(overlaps),
        "median_overlap": percentile(overlaps, 0.5) if overlaps else None,
        "mean_best_similarity": _mean([pair["mean_best_similarity"] for pair in pairs]),
        "matched_by_function": sum(int(pair["matched_by_function"]) for pair in pairs),
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
