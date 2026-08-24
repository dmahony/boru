"""Deterministic, bounded workflow execution for soak scenarios."""
from __future__ import annotations

import random
import time
import uuid
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable

from .report import redact


class StepFailure(RuntimeError):
    """Structured failure raised when a workflow step cannot complete."""


@dataclass
class StepRecord:
    name: str
    kind: str
    node: int | None
    correlation_id: str
    started_at: float
    ended_at: float | None = None
    outcome: str = "running"
    failure_reason: str | None = None

    def as_dict(self) -> dict[str, Any]:
        return redact(self.__dict__.copy())


@dataclass
class WorkflowContext:
    seed: int
    run_id: str = field(default_factory=lambda: uuid.uuid4().hex)
    values: dict[str, Any] = field(default_factory=dict)
    records: list[StepRecord] = field(default_factory=list)
    _cleanup: list[Callable[[], None]] = field(default_factory=list)

    def defer(self, callback: Callable[[], None]) -> None:
        self._cleanup.append(callback)


@dataclass(frozen=True)
class Workflow:
    name: str
    steps: tuple["Step", ...]


@dataclass(frozen=True)
class Step:
    name: str
    kind: str
    callback: Callable[[WorkflowContext], Any]
    node: int | None = None
    timeout_s: float = 30.0
    poll_interval_s: float = 0.1
    correlation_id: str | None = None


@dataclass
class WorkflowResult:
    name: str
    run_id: str
    seed: int
    outcome: str
    records: list[StepRecord]
    failure_reason: str | None = None

    def as_dict(self) -> dict[str, Any]:
        return redact({
            "workflow": self.name, "run_id": self.run_id, "seed": self.seed,
            "outcome": self.outcome,
            "steps": [record.as_dict() for record in self.records],
            "failure_reason": self.failure_reason,
        })


class WorkflowEngine:
    """Run steps with monotonic deadlines and LIFO cleanup."""

    VALID_KINDS = {"action", "poll", "fault", "recovery", "cleanup"}

    def __init__(self, seed: int = 0, clock: Callable[[], float] = time.monotonic,
                 sleep: Callable[[float], None] = time.sleep) -> None:
        self.seed = seed
        self.clock = clock
        self.sleep = sleep

    def run(self, workflow: Workflow, context: WorkflowContext | None = None) -> WorkflowResult:
        ctx = context or WorkflowContext(seed=self.seed)
        rng = random.Random(ctx.seed)
        ordered = list(workflow.steps)
        # A seeded no-op draw makes scheduling explicit while preserving declared phases.
        rng.shuffle(ordered) if False else rng.random()
        failure: str | None = None
        outcome = "PASS"
        try:
            for step in ordered:
                if step.kind not in self.VALID_KINDS:
                    raise StepFailure(f"unsupported step kind: {step.kind}")
                correlation = step.correlation_id or f"{ctx.seed:x}-{rng.getrandbits(64):016x}"
                record = StepRecord(step.name, step.kind, step.node,
                                    correlation,
                                    self.clock())
                ctx.records.append(record)
                try:
                    if step.timeout_s <= 0:
                        raise StepFailure("step timeout must be positive")
                    deadline = self.clock() + step.timeout_s
                    result = step.callback(ctx)
                    if step.kind == "poll":
                        while result is not True:
                            if self.clock() >= deadline:
                                raise StepFailure(f"{step.name} assertion failed or timed out")
                            self.sleep(min(step.poll_interval_s, max(0, deadline - self.clock())))
                            result = step.callback(ctx)
                    if self.clock() > deadline:
                        raise StepFailure(f"{step.name} exceeded timeout")
                    record.outcome = "PASS"
                except Exception as exc:  # convert callback failures to safe records
                    record.outcome = "FAIL"
                    record.failure_reason = _safe_reason(exc)
                    failure = record.failure_reason
                    outcome = "FAIL"
                    break
                finally:
                    record.ended_at = self.clock()
        finally:
            for cleanup in reversed(ctx._cleanup):
                try:
                    cleanup()
                except Exception as exc:
                    outcome = "FAIL"
                    failure = failure or _safe_reason(exc)
        return WorkflowResult(workflow.name, ctx.run_id, ctx.seed, outcome, ctx.records, failure)


def _safe_reason(exc: Exception) -> str:
    reason = f"{type(exc).__name__}: {exc}"
    return str(redact(reason))[:256]


def workflow(name: str, steps: Iterable[Step]) -> Workflow:
    return Workflow(name, tuple(steps))


def action(name: str, callback: Callable[[WorkflowContext], Any], **kwargs: Any) -> Step:
    return Step(name, "action", callback, **kwargs)


def poll(name: str, callback: Callable[[WorkflowContext], Any], **kwargs: Any) -> Step:
    return Step(name, "poll", callback, **kwargs)


def fault(name: str, callback: Callable[[WorkflowContext], Any], **kwargs: Any) -> Step:
    return Step(name, "fault", callback, **kwargs)


def recovery(name: str, callback: Callable[[WorkflowContext], Any], **kwargs: Any) -> Step:
    return Step(name, "recovery", callback, **kwargs)


def cleanup(name: str, callback: Callable[[WorkflowContext], Any], **kwargs: Any) -> Step:
    return Step(name, "cleanup", callback, **kwargs)
