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

    async def start(self) -> None:
        self.started = True

    def stop(self) -> None:
        self.watching = False

    def exit(self) -> None:
        self.exits += 1

    def back(self) -> None:
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
