"""A Bluetooth LE role stays registered with bluetoothd across its restarts.

bluetoothd holds everything a role registers (the discovery session, the GATT
application, the advertisement), and a new bluetoothd starts with none of it.
These pin the two cases that used to leave a device silent: a start cut short
by bluetoothd exiting, and a restart under a running role.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from offline_protocol_sdk.bluez_restarts import BlueZRestarts, StaysRegistered


def owner_changed(old: str, new: str) -> SimpleNamespace:
    return SimpleNamespace(member="NameOwnerChanged", body=["org.bluez", old, new])


class FakeWatch:
    """Stands in for the bus watch: `exit()` is bluetoothd leaving the bus,
    `back()` its successor claiming the name."""

    def __init__(self, on_restart, watching: bool = True) -> None:
        self._on_restart = on_restart
        self.watching = watching
        self.exits = 0
        self.started = False
        self._owned = asyncio.Event()
        self._owned.set()

    async def start(self) -> None:
        self.started = True

    def stop(self) -> None:
        self.watching = False

    async def wait_for_owner(self, timeout: float) -> None:
        try:
            await asyncio.wait_for(self._owned.wait(), timeout)
        except asyncio.TimeoutError:
            pass

    def exit(self) -> None:
        self.exits += 1
        self._owned.clear()

    def back(self) -> None:
        self._owned.set()
        self._on_restart()


def make(bring_up=None, tear_down=None, watching: bool = True):
    role = SimpleNamespace(
        bring_up=bring_up or AsyncMock(),
        tear_down=tear_down or AsyncMock(),
        emit=MagicMock(),
    )
    registration = StaysRegistered(role.bring_up, role.tear_down, role.emit)
    registration._watch = FakeWatch(registration._restarted, watching)
    registration.SETTLE = 0
    registration.OWNER_TIMEOUT = 1.0
    registration.SIGNAL_GRACE = 0
    registration.RECOVERY_DELAYS = (0, 0, 0, 0)
    return registration, role


class TestTheWatch:
    def test_an_exit_is_counted_and_a_new_owner_calls_back(self):
        calls = []
        watch = BlueZRestarts(lambda: calls.append(1))
        watch._observe(owner_changed(":1.57", ""))
        assert watch.exits == 1 and calls == []
        watch._observe(owner_changed("", ":1.61"))
        assert calls == [1]

    def test_other_names_and_signals_are_ignored(self):
        calls = []
        watch = BlueZRestarts(lambda: calls.append(1))
        watch._observe(SimpleNamespace(member="NameOwnerChanged", body=["org.other", ":1.2", ""]))
        watch._observe(SimpleNamespace(member="PropertiesChanged", body=["org.bluez", ":1.2", ""]))
        assert watch.exits == 0 and calls == []

    def test_an_adapter_powering_on_calls_back_without_counting_an_exit(self):
        """BlueZ drops every discovery session when the adapter powers off,
        and the peripheral power-cycles it to reset a wedged controller: the
        central in the same process must register again when it is back."""
        calls = []
        watch = BlueZRestarts(lambda: calls.append(1))

        def powered(value, interface="org.bluez.Adapter1"):
            return SimpleNamespace(
                member="PropertiesChanged",
                body=[interface, {"Powered": SimpleNamespace(value=value)}, []],
            )

        watch._observe(powered(False))
        assert calls == []
        watch._observe(powered(True))
        assert calls == [1] and watch.exits == 0
        # Another interface's Powered, or an adapter change without it, is not one.
        watch._observe(powered(True, interface="org.bluez.Device1"))
        watch._observe(SimpleNamespace(
            member="PropertiesChanged",
            body=["org.bluez.Adapter1", {"Discovering": SimpleNamespace(value=True)}, []],
        ))
        assert calls == [1]

    @pytest.mark.asyncio
    async def test_waiting_for_the_owner_returns_when_the_name_is_claimed(self):
        watch = BlueZRestarts(lambda: None)
        watch._observe(owner_changed(":1.57", ""))
        waiter = asyncio.ensure_future(watch.wait_for_owner(5))
        await asyncio.sleep(0)
        assert not waiter.done()
        watch._observe(owner_changed("", ":1.61"))
        await asyncio.wait_for(waiter, 1)

    @pytest.mark.asyncio
    async def test_waiting_for_an_owner_that_never_comes_gives_up(self):
        watch = BlueZRestarts(lambda: None)
        watch._observe(owner_changed(":1.57", ""))
        await watch.wait_for_owner(0.01)

    @pytest.mark.asyncio
    async def test_off_linux_it_never_touches_a_bus(self, monkeypatch):
        monkeypatch.setattr("sys.platform", "darwin")
        watch = BlueZRestarts(lambda: None)
        await watch.start()
        assert not watch.watching


class TestStart:
    @pytest.mark.asyncio
    async def test_a_start_cut_short_by_bluetoothd_exiting_is_tried_again(self):
        registration, role = make()

        async def bluetoothd_exits_mid_call():
            if role.bring_up.await_count == 1:
                registration._watch.exit()
                raise RuntimeError("Message recipient disconnected from message bus without replying")

        role.bring_up.side_effect = bluetoothd_exits_mid_call
        await registration.start()

        assert role.bring_up.await_count == 2
        role.tear_down.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_the_successor_claiming_the_name_does_not_also_start_a_recovery(self):
        """In the field the successor claims the name about 150 ms after the
        exit, while the start is still waiting to try again. Answering that
        claim with a recovery as well brought the role up twice at once, or
        made the start fail with "already running" while the recovery had the
        role up. The role here behaves like both real ones: it refuses while
        running, and the test counts bring-ups in flight."""
        registration, role = make()
        registration.SETTLE = 0.05
        registration.RECOVERY_DELAYS = (0.05,)
        state = {"running": False, "in_flight": 0, "most": 0}

        async def bring_up():
            if state["running"]:
                raise RuntimeError("already running")
            state["in_flight"] += 1
            state["most"] = max(state["most"], state["in_flight"])
            try:
                if role.bring_up.await_count == 1:
                    registration._watch.exit()
                    asyncio.get_running_loop().call_later(0.01, registration._watch.back)
                    raise RuntimeError("Message recipient disconnected from message bus without replying")
                await asyncio.sleep(0.02)
                state["running"] = True
            finally:
                state["in_flight"] -= 1

        async def tear_down():
            state["running"] = False

        role.bring_up.side_effect = bring_up
        role.tear_down.side_effect = tear_down

        await registration.start()
        await asyncio.sleep(0.2)  # long enough for any stray recovery to run

        assert registration._recovery is None
        assert state["most"] == 1
        assert state["running"]
        assert role.bring_up.await_count == 2

    @pytest.mark.asyncio
    async def test_the_retry_waits_for_the_successor_to_claim_the_name(self):
        registration, role = make()
        registration.OWNER_TIMEOUT = 5.0

        async def bluetoothd_exits_mid_call():
            if role.bring_up.await_count == 1:
                registration._watch.exit()
                raise RuntimeError("Message recipient disconnected")

        role.bring_up.side_effect = bluetoothd_exits_mid_call
        start = asyncio.ensure_future(registration.start())
        await asyncio.sleep(0.05)
        assert role.bring_up.await_count == 1  # still waiting for the claim
        registration._watch.back()
        await asyncio.wait_for(start, 1)
        assert role.bring_up.await_count == 2

    @pytest.mark.asyncio
    async def test_a_stop_while_a_start_waits_to_retry_wins(self):
        """stop() cannot cancel the retry (it runs inside the caller's
        start()), so the retry must see the stop and not bring the role up."""
        registration, role = make()
        registration.OWNER_TIMEOUT = 5.0

        async def bluetoothd_exits_mid_call():
            registration._watch.exit()
            raise RuntimeError("Message recipient disconnected")

        role.bring_up.side_effect = bluetoothd_exits_mid_call
        start = asyncio.ensure_future(registration.start())
        await asyncio.sleep(0.05)
        await registration.stop()
        registration._watch.back()

        with pytest.raises(RuntimeError, match="disconnected"):
            await asyncio.wait_for(start, 1)
        assert role.bring_up.await_count == 1
        assert not registration._wanted

    @pytest.mark.asyncio
    async def test_a_refusal_after_a_cut_short_attempt_is_raised(self):
        registration, role = make()

        async def exits_then_refuses():
            if role.bring_up.await_count == 1:
                registration._watch.exit()
                registration._watch.back()
                raise RuntimeError("Message recipient disconnected")
            raise RuntimeError("org.bluez.Error.NotPermitted")

        role.bring_up.side_effect = exits_then_refuses
        with pytest.raises(RuntimeError, match="NotPermitted"):
            await registration.start()
        assert role.bring_up.await_count == 2
        assert registration._recovery is None

    @pytest.mark.asyncio
    async def test_a_refusal_is_raised_at_once_and_not_retried(self):
        """No bluetoothd exit during the attempt: the start was refused (no
        adapter, no authorization), and trying again would only delay it."""
        registration, role = make(bring_up=AsyncMock(side_effect=RuntimeError("no adapter")))

        with pytest.raises(RuntimeError, match="no adapter"):
            await registration.start()

        assert role.bring_up.await_count == 1
        assert not registration._wanted
        assert not registration._watch.watching

    @pytest.mark.asyncio
    async def test_without_a_watch_a_failure_is_a_plain_failure(self):
        registration, role = make(bring_up=AsyncMock(side_effect=RuntimeError("x")), watching=False)
        with pytest.raises(RuntimeError):
            await registration.start()
        assert role.bring_up.await_count == 1

    @pytest.mark.asyncio
    async def test_retries_are_bounded(self):
        registration, role = make()

        async def bluetoothd_exits_every_time():
            registration._watch.exit()
            raise RuntimeError("Message recipient disconnected")

        role.bring_up.side_effect = bluetoothd_exits_every_time
        with pytest.raises(RuntimeError):
            await registration.start()
        assert role.bring_up.await_count == StaysRegistered.ATTEMPTS


class TestRestartUnderARunningRole:
    @pytest.mark.asyncio
    async def test_the_role_is_torn_down_and_registered_again(self):
        registration, role = make()
        await registration.start()

        registration._watch.exit()
        registration._watch.back()
        await registration._recovery

        role.tear_down.assert_awaited_once()
        assert role.bring_up.await_count == 2
        levels = [call.args[0] for call in role.emit.call_args_list]
        assert levels == ["warning", "info"]

    @pytest.mark.asyncio
    async def test_a_stopped_role_is_not_brought_back(self):
        registration, role = make()
        await registration.start()
        await registration.stop()

        registration._watch.back()

        assert registration._recovery is None
        assert role.bring_up.await_count == 1

    @pytest.mark.asyncio
    async def test_a_registration_refused_after_a_restart_is_tried_again(self):
        """A new bluetoothd refuses for a while ("InProgress" for a discovery
        it is still setting up): that is not a reason to stay unregistered."""
        registration, role = make()
        await registration.start()
        role.bring_up.side_effect = [RuntimeError("org.bluez.Error.InProgress"), None]

        registration._watch.back()
        await registration._recovery

        assert role.bring_up.await_count == 3
        assert role.emit.call_args_list[-1].args[0] == "info"

    @pytest.mark.asyncio
    async def test_a_stop_during_recovery_wins(self):
        """The owner's stop() must not be undone by a recovery that was
        waiting for bluetoothd to settle when it arrived."""
        registration, role = make()
        registration.RECOVERY_DELAYS = (10,)
        await registration.start()

        registration._watch.back()
        await asyncio.sleep(0)
        await registration.stop()

        assert role.bring_up.await_count == 1
        assert registration._recovery is None

    @pytest.mark.asyncio
    async def test_a_second_restart_during_recovery_is_folded_into_it(self):
        registration, role = make()
        registration.RECOVERY_DELAYS = (0.01,)
        await registration.start()

        registration._watch.back()
        first = registration._recovery
        registration._watch.back()

        assert registration._recovery is first
        await first
        assert role.bring_up.await_count == 2

    @pytest.mark.asyncio
    async def test_a_failed_registration_is_reported_not_raised(self):
        registration, role = make()
        await registration.start()
        role.bring_up.side_effect = RuntimeError("adapter gone")

        registration._watch.back()
        await registration._recovery

        assert role.emit.call_args_list[-1].args[0] == "error"
        assert role.bring_up.await_count == 1 + len(registration.RECOVERY_DELAYS)
