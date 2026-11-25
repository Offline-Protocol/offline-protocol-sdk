"""Token buckets: the per-device and per-peer-gateway budgets the contract
requires ("Abuse and exhaustion").

A gateway that does not bound what one attached device can consume hands that
device the backbone. On a scarce backbone (LoRa-class links, a duty cycle) one
chatty device starves every other device behind the same gateway, and a
device cannot tell from the outside that its gateway applies no budget at all.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Callable


@dataclass
class TokenBucket:
    """``rate`` tokens per second, up to ``burst`` held.

    ``take`` is all-or-nothing: a refused request costs nothing, so a device
    that backs off is not punished for the attempt.
    """

    rate: float
    burst: float
    clock: Callable[[], float] = time.monotonic
    _tokens: float = field(init=False)
    _at: float = field(init=False)

    def __post_init__(self) -> None:
        if self.rate < 0 or self.burst <= 0:
            raise ValueError("a bucket needs a non-negative rate and a positive burst")
        self._tokens = self.burst
        self._at = self.clock()

    def take(self, amount: float = 1.0) -> bool:
        if not self.has(amount):
            return False
        self._tokens -= amount
        return True

    def has(self, amount: float = 1.0) -> bool:
        """Whether ``take(amount)`` would succeed, spending nothing. Lets a
        caller check two buckets before charging either."""
        now = self.clock()
        self._tokens = min(self.burst, self._tokens + (now - self._at) * self.rate)
        self._at = now
        return amount <= self._tokens


class BucketMap:
    """One bucket per key, made on first use, with the oldest dropped past
    ``capacity`` so a stream of fresh keys cannot grow it without bound."""

    def __init__(
        self,
        rate: float,
        burst: float,
        capacity: int = 4096,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._rate = rate
        self._burst = burst
        self._capacity = capacity
        self._clock = clock
        self._buckets: dict[str, TokenBucket] = {}

    def take(self, key: str, amount: float = 1.0) -> bool:
        return self._bucket(key).take(amount)

    def has(self, key: str, amount: float = 1.0) -> bool:
        return self._bucket(key).has(amount)

    def _bucket(self, key: str) -> TokenBucket:
        bucket = self._buckets.pop(key, None)
        if bucket is None:
            bucket = TokenBucket(self._rate, self._burst, self._clock)
            while len(self._buckets) >= self._capacity:
                self._buckets.pop(next(iter(self._buckets)))
        # Re-inserted at the end: the dict's order is the use order.
        self._buckets[key] = bucket
        return bucket
