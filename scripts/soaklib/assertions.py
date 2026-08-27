"""Deterministic application and resource assertions for soak observations."""
from __future__ import annotations

import statistics
import time
from dataclasses import dataclass, field as dc_field
from typing import Any, Iterable, Mapping


Assertion = Any


@dataclass(frozen=True)
class MetricRule:
    """Limits applied after a warmup window.

    A limit of ``None`` disables that check.  Values are deliberately supplied
    by the caller: this module does not impose machine-specific thresholds.
    """

    metric: str
    max_final: float | None = None
    max_peak: float | None = None
    max_delta: float | None = None
    max_slope: float | None = None
    warmup_samples: int = 0
    min_samples: int = 1


@dataclass
class AssertionResult:
    name: str
    nodes: list[int | str]
    started_at: float
    ended_at: float
    expected: dict[str, Any]
    last_observed: dict[str, Any]
    status: str
    failure_code: str | None = None
    details: dict[str, Any] = dc_field(default_factory=dict)

    @property
    def timestamps(self) -> dict[str, float]:
        return {"started_at": self.started_at, "ended_at": self.ended_at}

    def as_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "nodes": list(self.nodes),
            "timestamps": self.timestamps,
            "expected": self.expected,
            "last_observed": self.last_observed,
            "status": self.status,
            "failure_code": self.failure_code,
            "details": self.details,
        }


def _result(name: str, nodes: Iterable[int | str], started: float,
            expected: dict[str, Any], observed: dict[str, Any], status: str,
            failure_code: str | None = None, details: dict[str, Any] | None = None,
            clock: Any = time.time) -> AssertionResult:
    return AssertionResult(name, list(nodes), started, clock(), expected, observed,
                           status, failure_code, details or {})


def evaluate_metric(name: str, samples: Iterable[float | int | None], rule: MetricRule,
                    nodes: Iterable[int | str] = (), *, started_at: float | None = None,
                    clock: Any = time.time) -> AssertionResult:
    """Evaluate one deterministic metric sequence and return PASS/FAIL/SKIP.

    Warmup samples establish a median baseline.  Missing values are not silently
    converted to zero: an entirely unsupported metric is SKIP, while a partially
    missing metric is a deterministic FAIL with ``metric_unavailable``.
    """
    started = clock() if started_at is None else started_at
    raw = list(samples)
    expected = {"metric": rule.metric, "warmup_samples": rule.warmup_samples,
                "limits": {k: v for k, v in {
                    "max_final": rule.max_final, "max_peak": rule.max_peak,
                    "max_delta": rule.max_delta, "max_slope": rule.max_slope,
                }.items() if v is not None}}
    if not raw or all(value is None for value in raw):
        return _result(name, nodes, started, expected, {"metric": rule.metric}, "SKIP",
                       "unsupported_metric", {"metric": rule.metric}, clock)
    if any(value is None for value in raw):
        return _result(name, nodes, started, expected, {"metric": rule.metric}, "FAIL",
                       "metric_unavailable", {"metric": rule.metric}, clock)
    values = [float(value) for value in raw if value is not None]
    if len(values) < rule.min_samples:
        return _result(name, nodes, started, expected, {"metric": rule.metric,
                       "sample_count": len(values)}, "FAIL", "insufficient_samples", clock=clock)
    warmup = values[:rule.warmup_samples]
    baseline = statistics.median(warmup) if warmup else values[0]
    measured = values[rule.warmup_samples:] or values
    final = measured[-1]
    peak = max(measured)
    delta = final - baseline
    slope = (measured[-1] - measured[0]) / max(1, len(measured) - 1)
    observed = {"metric": rule.metric, "baseline": baseline, "peak": peak,
                "final": final, "delta": delta, "slope": slope,
                "sample_count": len(values)}
    checks = (("peak_limit", rule.max_peak, peak), ("final_limit", rule.max_final, final),
              ("delta_limit", rule.max_delta, delta), ("slope_limit", rule.max_slope, slope))
    for code, limit, actual in checks:
        if limit is not None and actual > limit:
            return _result(name, nodes, started, expected, observed, "FAIL", code,
                           {"metric": rule.metric, "baseline": baseline, "peak": peak,
                            "final": final, "threshold": limit}, clock)
    return _result(name, nodes, started, expected, observed, "PASS", clock=clock)


def evaluate_metrics(observations: Mapping[str, Iterable[float | int | None]],
                     rules: Iterable[MetricRule], nodes: Iterable[int | str] = ()) -> list[AssertionResult]:
    return [evaluate_metric(rule.metric, observations.get(rule.metric, []), rule, nodes)
            for rule in rules]


class AssertionEngine:
    """Collect application-level assertion results for a soak run."""

    def __init__(self, clock: Any = time.time) -> None:
        self.clock = clock
        self.results: list[AssertionResult] = []

    def metric(self, name: str, samples: Iterable[float | int | None], rule: MetricRule,
               nodes: Iterable[int | str] = ()) -> AssertionResult:
        result = evaluate_metric(name, samples, rule, nodes, clock=self.clock)
        self.results.append(result)
        return result

    def orphan_children(self, name: str, children: Iterable[int] | None,
                        nodes: Iterable[int | str] = ()) -> AssertionResult:
        started = self.clock()
        if children is None:
            result = _result(name, nodes, started, {"orphan_children": 0}, {}, "SKIP",
                             "unsupported_metric", {"metric": "orphan_children"}, self.clock)
        else:
            child_list = list(children)
            observed = {"orphan_children": child_list, "count": len(child_list)}
            result = _result(name, nodes, started, {"orphan_children": 0}, observed,
                             "PASS" if not child_list else "FAIL",
                             None if not child_list else "orphan_children", clock=self.clock)
        self.results.append(result)
        return result

    def passed(self) -> bool:
        return all(result.status in {"PASS", "SKIP"} for result in self.results)


def equals(expected: Any) -> Assertion:
    return lambda actual: actual == expected


def field(name: str, expected: Any = ...) -> Assertion:
    def check(value: Any) -> bool:
        return isinstance(value, dict) and name in value and (expected is ... or value[name] == expected)
    return check


def assert_true(value: Any, message: str = "assertion failed") -> None:
    if not value:
        raise AssertionError(message)
