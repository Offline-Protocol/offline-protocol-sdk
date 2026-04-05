"""Base transport manager abstraction.

Mirrors the iOS ``TransportManager`` protocol and Android
``TransportManager`` interface so that each transport (BLE, Internet, etc.)
follows the same lifecycle contract.
"""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from enum import Enum
from typing import Any, Callable

logger = logging.getLogger(__name__)


class TransportState(Enum):
    """Transport lifecycle states."""

    UNAVAILABLE = "unavailable"
    AVAILABLE = "available"
    STARTING = "starting"
    RUNNING = "running"
    STOPPING = "stopping"
    STOPPED = "stopped"


class TransportError(Exception):
    """Base error for transport operations."""


class TransportManager(ABC):
    """Abstract base for all transport implementations.

    Subclasses must implement :meth:`is_available`, :meth:`start`, and
    :meth:`stop`.  Optional hooks :meth:`pause`, :meth:`resume`, and
    :meth:`get_metrics` have sensible defaults.
    """

    transport_id: str
    transport_name: str

    def __init__(self) -> None:
        self._state = TransportState.STOPPED
        self._on_state_change: Callable[[TransportState], None] | None = None
        self._on_error: Callable[[Exception], None] | None = None
        self._on_diagnostic: (
            Callable[[str, str, dict[str, Any]], None] | None
        ) = None

    @property
    def state(self) -> TransportState:
        return self._state

    def set_delegate(
        self,
        *,
        on_state_change: Callable[[TransportState], None] | None = None,
        on_error: Callable[[Exception], None] | None = None,
        on_diagnostic: (
            Callable[[str, str, dict[str, Any]], None] | None
        ) = None,
    ) -> None:
        """Set delegate callbacks (equivalent to iOS TransportManagerDelegate)."""
        self._on_state_change = on_state_change
        self._on_error = on_error
        self._on_diagnostic = on_diagnostic

    @abstractmethod
    def is_available(self) -> bool:
        """Return True if transport hardware/capabilities are available."""
        ...

    @abstractmethod
    async def start(self) -> None:
        """Start the transport. Raises :class:`TransportError` on failure."""
        ...

    @abstractmethod
    async def stop(self) -> None:
        """Stop the transport gracefully."""
        ...

    async def pause(self) -> None:
        """Pause the transport (default: stop)."""
        await self.stop()

    async def resume(self) -> None:
        """Resume the transport from paused state (default: start)."""
        await self.start()

    def get_metrics(self) -> dict[str, Any]:
        """Return current transport metrics."""
        return {}

    # -- helpers for subclasses ------------------------------------------------

    def _update_state(self, new_state: TransportState) -> None:
        old = self._state
        self._state = new_state
        if old != new_state:
            logger.debug(
                "%s state: %s -> %s",
                self.transport_id,
                old.value,
                new_state.value,
            )
            if self._on_state_change is not None:
                self._on_state_change(new_state)

    def _emit_error(self, exc: Exception) -> None:
        logger.error("%s error: %s", self.transport_id, exc)
        if self._on_error is not None:
            self._on_error(exc)

    def _emit_diagnostic(
        self, level: str, message: str, context: dict[str, Any] | None = None
    ) -> None:
        ctx = context or {}
        logger.log(
            _LOG_LEVELS.get(level, logging.DEBUG),
            "%s: %s %s",
            self.transport_id,
            message,
            ctx if ctx else "",
        )
        if self._on_diagnostic is not None:
            self._on_diagnostic(level, message, ctx)


_LOG_LEVELS: dict[str, int] = {
    "error": logging.ERROR,
    "warning": logging.WARNING,
    "warn": logging.WARNING,
    "info": logging.INFO,
    "debug": logging.DEBUG,
}
