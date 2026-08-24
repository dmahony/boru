"""Deterministic, state-aware fault scheduling primitives."""
from __future__ import annotations

import hashlib
import random
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable, Mapping


class FaultSchedulingError(RuntimeError):
    """Base class for invalid or impossible fault schedules."""


class InvalidFaultTarget(FaultSchedulingError):
    """Raised when a schedule names a node no longer owned by the run."""


@dataclass(frozen=True)
class FaultSpec:
    """A fault requested at a semantic workflow point."""

    kind: str
    semantic_step: str = "periodic"
    target: int | None = None
    recovery_window_s: float = 30.0
    ordinal: int = 0


@dataclass(frozen=True)
class ScheduledFault:
    fault_id: str
    kind: str
    selected_node: int
    semantic_step: str
    recovery_window_s: float
    ordinal: int

    def as_dict(self) -> dict[str, Any]:
        return {
            "fault_id": self.fault_id,
            "fault": self.kind,
            "node": self.selected_node,
            "semantic_step": self.semantic_step,
            "recovery_window_s": self.recovery_window_s,
            "ordinal": self.ordinal,
        }


@dataclass
class FaultScheduler:
    """Create reproducible schedules and release faults at semantic points.

    Node selection is independent of process state.  ``active_nodes`` is checked
    at scheduling/trigger time so a cleaned-up node fails clearly instead of
    silently receiving a fault.
    """

    seed: int
    workflow_name: str
    active_nodes: set[int]
    _scheduled: list[ScheduledFault] = field(default_factory=list)
    _triggered: set[str] = field(default_factory=set)

    def schedule(self, specs: Iterable[FaultSpec]) -> list[ScheduledFault]:
        rng = random.Random(self.seed)
        result: list[ScheduledFault] = []
        nodes = sorted(self.active_nodes)
        if not nodes:
            raise InvalidFaultTarget("cannot schedule a fault: no owned nodes remain")
        for ordinal, spec in enumerate(specs):
            if spec.recovery_window_s <= 0:
                raise FaultSchedulingError("recovery window must be positive")
            if spec.target is not None:
                if spec.target not in self.active_nodes:
                    raise InvalidFaultTarget(
                        f"fault target node {spec.target} is not an active owned node"
                    )
                selected = spec.target
            else:
                selected = nodes[rng.randrange(len(nodes))]
            token = f"{self.workflow_name}|{self.seed}|{ordinal}|{spec.kind}|{spec.semantic_step}|{selected}"
            fault_id = hashlib.sha256(token.encode("utf-8")).hexdigest()[:16]
            result.append(ScheduledFault(
                fault_id, spec.kind, selected, spec.semantic_step,
                spec.recovery_window_s, spec.ordinal,
            ))
        self._scheduled = result
        return list(result)

    def trigger(
        self,
        semantic_step: str,
        state: Mapping[str, Any] | None = None,
        snapshot: Callable[[int], Mapping[str, Any]] | None = None,
    ) -> list[dict[str, Any]]:
        """Trigger all pending faults matching ``semantic_step``.

        Each record includes before/after snapshots.  Callers can pass a
        snapshot function backed by MCP/procfs; a state mapping is convenient
        for transport-free tests.
        """
        state = state or {}
        records: list[dict[str, Any]] = []
        for fault in self._scheduled:
            if fault.fault_id in self._triggered or fault.semantic_step != semantic_step:
                continue
            if fault.selected_node not in self.active_nodes:
                raise InvalidFaultTarget(
                    f"fault {fault.fault_id} targets cleaned-up node {fault.selected_node}"
                )
            before = dict(snapshot(fault.selected_node)) if snapshot else dict(state)
            record = fault.as_dict()
            record["before"] = before
            after = dict(snapshot(fault.selected_node)) if snapshot else dict(state)
            record["after"] = after
            self._triggered.add(fault.fault_id)
            records.append(record)
        return records

    @property
    def scheduled(self) -> tuple[ScheduledFault, ...]:
        return tuple(self._scheduled)

    @property
    def triggered(self) -> frozenset[str]:
        return frozenset(self._triggered)
