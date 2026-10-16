"""Tells a Bluetooth LE role when bluetoothd restarts under it.

Everything either role registers with BlueZ lives in the bluetoothd process:
the central's discovery session, the peripheral's GATT application and its
advertisement. When bluetoothd exits and systemd starts a new one, all of it
is gone, and nothing on the bus says so to the process that registered it.
Before this existed both roles kept reporting themselves running: the
peripheral logged "advertising" with no advertisement registered and the
central waited on a scan that had ended, so the device dropped off the air
until its service was restarted by hand.

The one signal that does arrive is the bus's own: ``NameOwnerChanged`` for
``org.bluez``, which moves to an empty owner when bluetoothd exits and to a
new one when its successor claims the name. This watches for that and calls
``on_restart`` once the name has a new owner, which is when registering
again can succeed.

An adapter powered off and on again is the same loss for the central: BlueZ
drops every discovery session when the adapter powers off, and the scan does
not come back with the power. So ``on_restart`` is also called when an
adapter's ``Powered`` turns true. The peripheral powers the adapter off and on
itself to reset a controller that refuses every advertisement (see
``ble_peripheral._reset_wedged_advertising``); without this the central in
the same process stopped hearing anyone from then on.

Linux only, and best effort: where there is no system bus to watch (a test
host, a container without the grant) it logs and stays inert, which is the
behaviour that existed before it.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
import sys
import weakref
from typing import Any, Callable

logger = logging.getLogger(__name__)

_MATCH = "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.bluez'"
_POWER_MATCH = (
    "type='signal',sender='org.bluez',interface='org.freedesktop.DBus.Properties',"
    "member='PropertiesChanged',arg0='org.bluez.Adapter1'"
)


class BlueZRestarts:
    """Counts bluetoothd's exits and calls ``on_restart`` when it is back, or
    when an adapter powers back on."""

    def __init__(self, on_restart: Callable[[], None]) -> None:
        self._on_restart = on_restart
        self._bus: Any = None
        #: How many times bluetoothd has left the bus since start(). A start
        #: that fails reads it before and after to tell a refusal from a
        #: registration cut short by bluetoothd exiting part-way.
        self.exits = 0
        # Set while org.bluez has an owner. Assumed owned until an exit is
        # seen: the watch starts while the role is about to register.
        self._owned = asyncio.Event()
        self._owned.set()

    async def start(self) -> None:
        if sys.platform != "linux" or self._bus is not None:
            return
        try:
            from dbus_fast import BusType, Message
            from dbus_fast.aio import MessageBus

            bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
            for rule in (_MATCH, _POWER_MATCH):
                await bus.call(
                    Message(
                        destination="org.freedesktop.DBus",
                        path="/org/freedesktop/DBus",
                        interface="org.freedesktop.DBus",
                        member="AddMatch",
                        signature="s",
                        body=[rule],
                    )
                )
        except Exception as exc:
            logger.info("bluetoothd restarts are not watched here: %s", exc)
            return
        bus.add_message_handler(self._observe)
        self._bus = bus

    @property
    def watching(self) -> bool:
        return self._bus is not None

    async def wait_for_owner(self, timeout: float) -> None:
        """Return once ``org.bluez`` has an owner again, or after ``timeout``."""
        try:
            await asyncio.wait_for(self._owned.wait(), timeout)
        except asyncio.TimeoutError:
            logger.warning("bluetoothd did not come back within %.0f s", timeout)

    def stop(self) -> None:
        bus, self._bus = self._bus, None
        if bus is None:
            return
        try:
            bus.remove_message_handler(self._observe)
            bus.disconnect()
        except Exception:
            logger.debug("closing the bluetoothd watch failed", exc_info=True)

    def _observe(self, message: Any) -> None:
        if _powered_on(message):
            logger.warning("the Bluetooth adapter powered on again; what was registered with it is gone")
            self._call_back()
            return
        if message.member != "NameOwnerChanged" or not message.body or message.body[0] != "org.bluez":
            return
        _, old_owner, new_owner = message.body
        if old_owner and not new_owner:
            self.exits += 1
            self._owned.clear()
            logger.warning("bluetoothd left the bus; what was registered with it is gone")
        elif new_owner:
            self._owned.set()
            self._call_back()

    def _call_back(self) -> None:
        try:
            self._on_restart()
        except Exception:
            logger.exception("handling a bluetoothd restart failed")


def _powered_on(message: Any) -> bool:
    """Whether ``message`` says an adapter's ``Powered`` turned true."""
    if message.member != "PropertiesChanged" or not message.body:
        return False
    if message.body[0] != "org.bluez.Adapter1" or len(message.body) < 2:
        return False
    changed = message.body[1]
    if not isinstance(changed, dict) or "Powered" not in changed:
        return False
    powered = changed["Powered"]
    return bool(getattr(powered, "value", powered))


class StaysRegistered:
    """Keeps one Bluetooth LE role registered with bluetoothd across restarts.

    ``bring_up`` registers the role and ``tear_down`` releases it; they are
    the role's own start and stop. Two cases differ from a plain call:

    - A start cut short because bluetoothd exited part-way (BlueZ answers
      "Message recipient disconnected from message bus without replying") is
      retried once the new bluetoothd is up. A start refused for any other
      reason is raised at once, as before.
    - A role that is running when bluetoothd restarts, or when an adapter
      powers on again, is torn down and brought up again, so it is
      registered with the process and the adapter as they are now.

    Only one of the two ever drives the role. A start cut short by an exit
    is followed, about 150 ms later, by the successor claiming the name; that
    claim must not also start a recovery, or the role is brought up twice at
    once (two scanners, or two copies of the GATT service). While a start is
    in progress the start owns the role, and it waits on the claim itself.

    Off Linux, or with no system bus to watch, both are plain calls.
    """

    #: Starts tried per bring-up when bluetoothd exits during them.
    ATTEMPTS = 3
    #: Seconds a start cut short waits for a new bluetoothd to claim the
    #: name before trying again regardless.
    OWNER_TIMEOUT = 10.0
    #: Seconds to let a new bluetoothd finish starting before registering.
    SETTLE = 1.0
    #: Waits before each registration after a restart; four tries over
    #: about fifteen seconds.
    RECOVERY_DELAYS = (1.0, 2.0, 4.0, 8.0)
    #: Seconds a failed start waits for bluetoothd's exit signal, which
    #: arrives with the failed call, before treating it as a refusal.
    SIGNAL_GRACE = 0.2

    def __init__(
        self,
        bring_up: Callable[[], Any],
        tear_down: Callable[[], Any],
        emit: Callable[[str, str, dict[str, Any] | None], None],
    ) -> None:
        # Held weakly: the role holds this object, and a cycle through it
        # would keep a dropped ProtocolManager (and the stores it holds open)
        # alive until the cycle collector ran, so the next manager over the
        # same directories would be refused.
        self._bring_up = _weakly(bring_up)
        self._tear_down = _weakly(tear_down)
        self._emit = _weakly(emit)
        self._watch = BlueZRestarts(self._restarted)
        self._wanted = False
        # True while start() is bringing the role up: a restart seen then is
        # the start's to handle, not a recovery's.
        self._starting = False
        self._recovery: Any = None

    async def start(self) -> None:
        self._wanted = True
        await self._watch.start()
        try:
            await self._bring_up_with_retries()
        except BaseException:
            self._wanted = False
            self._watch.stop()
            raise

    async def stop(self) -> None:
        self._wanted = False
        recovery, self._recovery = self._recovery, None
        if recovery is not None and not recovery.done():
            recovery.cancel()
            await asyncio.wait({recovery})
        self._watch.stop()
        await self._tear_down()

    async def _bring_up_with_retries(self) -> None:
        self._starting = True
        try:
            for attempt in range(1, self.ATTEMPTS + 1):
                exits = self._watch.exits
                try:
                    await self._bring_up()
                    return
                except Exception as exc:
                    if not self._watch.watching or attempt == self.ATTEMPTS:
                        raise
                    # The exit signal and the failed call arrive together;
                    # give the signal a moment before deciding which this was.
                    await asyncio.sleep(self.SIGNAL_GRACE)
                    if self._watch.exits == exits:
                        raise
                    self._emit("warning", "bluetoothd exited during the start; starting again", {
                        "attempt": attempt,
                        "error": str(exc),
                    })
                    await self._tear_down()
                    await self._watch.wait_for_owner(self.OWNER_TIMEOUT)
                    await asyncio.sleep(self.SETTLE)
                    # stop() cannot cancel this path (it runs inside the
                    # caller's start()), so it is checked for here: a role
                    # the owner stopped while this waited stays stopped, and
                    # the start reports that it did not succeed.
                    if not self._wanted:
                        raise
        finally:
            self._starting = False

    def _restarted(self) -> None:
        if not self._wanted or self._starting:
            return
        if self._recovery is not None and not self._recovery.done():
            return
        self._recovery = asyncio.ensure_future(self._register_again())

    async def _register_again(self) -> None:
        self._emit("warning", "bluetoothd restarted or the adapter powered on again; registering again", None)
        await self._tear_down()
        # A bluetoothd that has just started can refuse a registration for a
        # while (its adapter still powering up, or "InProgress" for a
        # discovery it is still setting up), so every failure here is tried
        # again with backoff, not only one cut short by another exit. stop()
        # cancels this task, so nothing here re-checks _wanted.
        error: Exception | None = None
        for delay in self.RECOVERY_DELAYS:
            await asyncio.sleep(delay)
            try:
                await self._bring_up()
            except Exception as exc:
                error = exc
                await self._tear_down()
            else:
                self._emit("info", "registered with bluetoothd again", None)
                return
        # Nothing tries again after this until bluetoothd restarts once more
        # or the owner restarts the role: the error is the operator's signal.
        self._emit("error", "could not register with bluetoothd again", {"error": str(error)})


def _weakly(fn: Callable[..., Any]) -> Callable[..., Any]:
    """``fn``, holding its object weakly when it is a bound method; a call
    after the object is gone does nothing."""
    if not inspect.ismethod(fn):
        return fn
    ref = weakref.WeakMethod(fn)
    is_coroutine = inspect.iscoroutinefunction(fn)
    del fn  # the closure must not hold it

    def call(*args: Any) -> Any:
        target = ref()
        if target is None:
            return _gone() if is_coroutine else None
        return target(*args)

    return call


async def _gone() -> None:
    return None
