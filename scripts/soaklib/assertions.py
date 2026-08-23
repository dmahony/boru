"""Small, composable assertions for workflow polling."""
from __future__ import annotations

from typing import Any, Callable


Assertion = Callable[[Any], bool]


def equals(expected: Any) -> Assertion:
    return lambda actual: actual == expected


def field(name: str, expected: Any = ... ) -> Assertion:
    def check(value: Any) -> bool:
        if not isinstance(value, dict) or name not in value:
            return False
        return expected is ... or value[name] == expected
    return check


def assert_true(value: Any, message: str = "assertion failed") -> None:
    if not value:
        raise AssertionError(message)
