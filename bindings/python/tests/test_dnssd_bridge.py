"""Tests for the DNS-SD bridge under ``docs/spec/dns-sd-mapping.md``.

No network and no responder: the mapping is tested as pure functions on
bytes, and the bridge is driven through a fake of the five calls it makes on
the responder. The chapter's bounds are asserted as literals, not read from
the module, for the reason the bridge rules give (C5).
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any
from unittest.mock import MagicMock

import pytest

from offline_protocol_sdk.dnssd_bridge import (
    ADDRESS_PREFIX,
    DEFAULT_TTL,
    INSTANCE_PREFIX,
    MAX_INSTANCE_NAME_OCTETS,
    MAX_SID_BYTES,
    MAX_TXT_RECORD_BYTES,
    MAX_TXT_STRING_BYTES,
    SUBTYPE,
    TXT_VERSION,
    DnsSdBridge,
    RecordRefused,
    ZeroconfBackend,
    instance_label,
    lan_service_event,
    parse_txt,
    record_from_txt,
    service_instance_name,
    txt_record,
    txt_record_size,
)
from offline_protocol_sdk.peer_stream_manager import SERVICE_TYPE, service_host_name
from offline_protocol_sdk.services import SOURCE_LAN, ServiceRecord, Services

OUR_ADDRESS = "off1qyulwy7s5ezz20cy222zrw04rwds39uapqv9j8r0"
PEER_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"


def encode_txt(entries: dict[str, str]) -> bytes:
    """The wire form of a TXT record: one length octet per string."""
    out = bytearray()
    for key, value in entries.items():
        string = f"{key}={value}".encode("utf-8")
        out.append(len(string))
        out += string
    return bytes(out)


def raw_strings(strings: list[bytes]) -> bytes:
    out = bytearray()
    for string in strings:
        out.append(len(string))
        out += string
    return bytes(out)


class FakeBackend:
    """The responder's five calls, remembered."""

    def __init__(self) -> None:
        self.registered: dict[str, tuple[object, dict[str, Any]]] = {}
        self.unregistered: list[object] = []
        self.on_change: Any = None
        self.txt_by_name: dict[str, bytes] = {}
        self.resolve_calls: list[str] = []
        self.closed = False

    async def register(self, **kw: Any) -> object:
        handle = object()
        self.registered[kw["name"]] = (handle, kw)
        return handle

    async def unregister(self, handle: object) -> None:
        for name, (held, _) in list(self.registered.items()):
            if held is handle:
                del self.registered[name]
        self.unregistered.append(handle)

    def browse(self, on_change: Any) -> None:
        self.on_change = on_change

    async def resolve(self, name: str, timeout_ms: int) -> bytes | None:
        self.resolve_calls.append(name)
        return self.txt_by_name.get(name)

    async def close(self) -> None:
        self.closed = True


@pytest.fixture
def mesh() -> MagicMock:
    m = MagicMock()
    m.unregister_service = MagicMock(return_value=True)
    return m


@pytest.fixture
def services(mesh: MagicMock) -> Services:
    return Services(MagicMock(), mesh_services=mesh)


@pytest.fixture
def backend() -> FakeBackend:
    return FakeBackend()


@pytest.fixture
def events() -> list[dict[str, Any]]:
    return []


@pytest.fixture
async def bridge(services, backend, events):
    b = DnsSdBridge(services, on_event=events.append, backend=backend, ttl=100.0)
    await b.start(address=OUR_ADDRESS, port=7878, addresses=["192.168.1.9"])
    try:
        yield b
    finally:
        await b.stop()


async def settle() -> None:
    """Let the tasks the listener scheduled run."""
    for _ in range(3):
        await asyncio.sleep(0)


# ---------------------------------------------------------------------------
# The chapter's literals
# ---------------------------------------------------------------------------


class TestChapterLiterals:
    def test_the_type_the_subtype_and_the_version(self):
        assert SERVICE_TYPE == "_offlineprotocol._tcp.local."
        assert len("_offlineprotocol") == 16  # underscore plus fifteen, the RFC 6763 bound
        assert SUBTYPE == "_svc._sub._offlineprotocol._tcp.local."
        assert TXT_VERSION == "1"
        assert INSTANCE_PREFIX == "svc-"
        assert ADDRESS_PREFIX == "off1"

    def test_the_bounds(self):
        assert MAX_INSTANCE_NAME_OCTETS == 63
        assert MAX_TXT_STRING_BYTES == 255
        assert MAX_SID_BYTES == 200
        assert MAX_TXT_RECORD_BYTES == 1300
        assert DEFAULT_TTL == 300.0


# ---------------------------------------------------------------------------
# The instance name
# ---------------------------------------------------------------------------


class TestInstanceName:
    def test_is_a_digest_of_address_and_id(self):
        label = instance_label(OUR_ADDRESS, "weather.v1")
        assert label.startswith("svc-") and len(label) == 20
        assert all(c in "0123456789abcdef" for c in label[4:])
        assert len(label.encode("utf-8")) <= 63
        assert instance_label(OUR_ADDRESS, "weather.v1") == label
        assert instance_label(PEER_ADDRESS, "weather.v1") != label
        assert instance_label(OUR_ADDRESS, "weather.v2") != label

    def test_the_separator_keeps_address_and_id_apart(self):
        # Without the 0x00 between them, "ab" + "c" and "a" + "bc" collide.
        assert instance_label("off1ab", "c") != instance_label("off1a", "bc")

    def test_the_full_name_is_under_the_type_and_carries_neither_input(self):
        name = service_instance_name(OUR_ADDRESS, "weather.v1")
        assert name.endswith("." + SERVICE_TYPE)
        assert OUR_ADDRESS not in name and "weather" not in name


# ---------------------------------------------------------------------------
# The TXT record, publisher side
# ---------------------------------------------------------------------------


class TestTxtRecord:
    def test_the_entries_and_their_order(self):
        record = ServiceRecord("weather.v1", "2.0", {"format": "json", "coverage": "us,eu"})
        entries = txt_record(record, OUR_ADDRESS)
        assert list(entries.items()) == [
            ("txtvers", "1"),
            ("sid", "weather.v1"),
            ("ver", "2.0"),
            ("addr", OUR_ADDRESS),
            ("c.coverage", "us,eu"),
            ("c.format", "json"),
        ]

    def test_capability_keys_are_in_byte_order(self):
        entries = txt_record(ServiceRecord("s", "", {"b": "1", "B": "2", "a": "3"}), OUR_ADDRESS)
        assert [k for k in entries if k.startswith("c.")] == ["c.B", "c.a", "c.b"]

    def test_the_size_counts_a_length_octet_per_string(self):
        entries = {"txtvers": "1", "sid": "s"}
        assert txt_record_size(entries) == (1 + len("txtvers=1")) + (1 + len("sid=s"))
        assert txt_record_size(entries) == len(encode_txt(entries))

    def test_a_service_id_at_the_bound_fits_and_one_over_is_refused(self):
        assert txt_record(ServiceRecord("a" * 200), OUR_ADDRESS)["sid"] == "a" * 200
        with pytest.raises(RecordRefused, match="201 bytes"):
            txt_record(ServiceRecord("a" * 201), OUR_ADDRESS)

    def test_the_id_bound_is_in_bytes_not_characters(self):
        with pytest.raises(RecordRefused):
            txt_record(ServiceRecord("é" * 101), OUR_ADDRESS)  # 202 bytes

    def test_a_record_at_the_bound_fits_and_one_over_is_refused(self):
        fixed = txt_record_size(txt_record(ServiceRecord("s"), OUR_ADDRESS))
        # Fill with 250-byte strings ("c.kN=" plus value) up to the bound.
        caps: dict[str, str] = {}
        remaining = 1300 - fixed
        n = 0
        while remaining >= 1 + 250:
            caps[f"k{n:02d}"] = "v" * (250 - len(f"c.k{n:02d}=") )
            remaining -= 1 + 250
            n += 1
        caps["last"] = "v" * (remaining - 1 - len("c.last="))
        entries = txt_record(ServiceRecord("s", "", caps), OUR_ADDRESS)
        assert txt_record_size(entries) == 1300
        caps["last"] += "v"
        with pytest.raises(RecordRefused, match="1301 bytes"):
            txt_record(ServiceRecord("s", "", caps), OUR_ADDRESS)

    def test_a_string_over_255_bytes_is_refused_not_shortened(self):
        record = ServiceRecord("s", "", {"k": "v" * (255 - len("c.k=") + 1)})
        with pytest.raises(RecordRefused, match="256 bytes"):
            txt_record(record, OUR_ADDRESS)
        assert txt_record(ServiceRecord("s", "", {"k": "v" * (255 - len("c.k="))}), OUR_ADDRESS)

    def test_a_version_over_the_string_bound_is_refused(self):
        with pytest.raises(RecordRefused, match="'ver'"):
            txt_record(ServiceRecord("s", "9" * 252), OUR_ADDRESS)

    @pytest.mark.parametrize("key", ["", "a=b", "tab\tkey", "é", "\x7f"])
    def test_a_capability_key_dns_sd_cannot_carry_is_refused(self, key):
        with pytest.raises(RecordRefused):
            txt_record(ServiceRecord("s", "", {key: "v"}), OUR_ADDRESS)

    def test_a_capability_value_may_be_any_utf8(self):
        entries = txt_record(ServiceRecord("s", "", {"lang": "français=oui"}), OUR_ADDRESS)
        assert entries["c.lang"] == "français=oui"

    def test_an_address_that_is_not_one_is_refused(self):
        with pytest.raises(RecordRefused, match="canonical address"):
            txt_record(ServiceRecord("s"), "alice")


# ---------------------------------------------------------------------------
# The TXT record, reader side
# ---------------------------------------------------------------------------


class TestParseTxt:
    def test_round_trip(self):
        entries = {"txtvers": "1", "sid": "weather.v1", "ver": "", "addr": PEER_ADDRESS, "c.f": "json"}
        assert parse_txt(encode_txt(entries)) == [(k.encode(), v.encode()) for k, v in entries.items()]

    def test_a_key_without_a_value_and_an_empty_string(self):
        assert parse_txt(raw_strings([b"flag", b"", b"k="])) == [(b"flag", None), (b"k", b"")]

    def test_a_length_past_the_end_is_malformed(self):
        assert parse_txt(b"\x05ab") is None
        assert parse_txt(b"") == []


class TestRecordFromTxt:
    def good(self, **overrides: Any) -> dict[str, str]:
        entries = {"txtvers": "1", "sid": "weather.v1", "ver": "2.0", "addr": PEER_ADDRESS, "c.format": "json"}
        entries.update(overrides)
        return entries

    def test_a_well_formed_record_is_a_lan_claim(self):
        record = record_from_txt(encode_txt(self.good()))
        assert record == ServiceRecord(
            "weather.v1", "2.0", {"format": "json"}, provider=PEER_ADDRESS, source=SOURCE_LAN
        )
        assert not record.is_local

    def test_the_version_defaults_to_empty(self):
        entries = self.good()
        del entries["ver"]
        assert record_from_txt(encode_txt(entries)).version == ""

    @pytest.mark.parametrize("txtvers", ["0", "2", ""])
    def test_another_mapping_version_is_ignored(self, txtvers):
        assert record_from_txt(encode_txt(self.good(txtvers=txtvers))) is None

    def test_txtvers_need_not_be_first_to_be_read(self):
        entries = {"sid": "weather.v1", "addr": PEER_ADDRESS, "txtvers": "1"}
        assert record_from_txt(encode_txt(entries)) is not None

    def test_missing_sid_or_addr_is_ignored(self):
        for key in ("sid", "addr"):
            entries = self.good()
            del entries[key]
            assert record_from_txt(encode_txt(entries)) is None
        assert record_from_txt(encode_txt(self.good(sid=""))) is None

    def test_an_addr_that_is_not_an_address_is_ignored(self):
        assert record_from_txt(encode_txt(self.good(addr="alice"))) is None

    def test_a_key_twice_is_ignored_whole(self):
        raw = raw_strings([b"txtvers=1", b"sid=a", b"sid=b", b"addr=" + PEER_ADDRESS.encode()])
        assert record_from_txt(raw) is None

    def test_a_value_that_is_not_utf8_is_ignored(self):
        raw = raw_strings([b"txtvers=1", b"sid=\xff\xfe", b"addr=" + PEER_ADDRESS.encode()])
        assert record_from_txt(raw) is None

    def test_a_key_dns_sd_forbids_is_ignored(self):
        raw = raw_strings([b"txtvers=1", b"sid=a", b"addr=" + PEER_ADDRESS.encode(), b"\x01=x"])
        assert record_from_txt(raw) is None

    def test_unknown_keys_and_an_empty_capability_key_are_skipped(self):
        record = record_from_txt(encode_txt(self.good(**{"other": "x", "c.": "y"})))
        assert record.capabilities == {"format": "json"}

    def test_the_reader_holds_the_publishers_bounds(self):
        assert record_from_txt(encode_txt(self.good(sid="a" * 201))) is None
        assert record_from_txt(encode_txt(self.good(sid="a" * 200))) is not None
        big = self.good(**{f"c.k{n}": "v" * 240 for n in range(6)})
        assert len(encode_txt(big)) > 1300
        assert record_from_txt(encode_txt(big)) is None

    def test_a_malformed_record_is_ignored(self):
        assert record_from_txt(b"\x09txtvers=1\x0fsid") is None


class TestLanEvent:
    def test_the_shape(self):
        record = ServiceRecord("weather.v1", "2.0", {"f": "j"}, provider=PEER_ADDRESS, source=SOURCE_LAN)
        assert lan_service_event(record) == {
            "type": "service_discovered",
            "source": "lan",
            "query_id": "",
            "service_id": "weather.v1",
            "version": "2.0",
            "provider_peer_id": PEER_ADDRESS,
            "capabilities": {"f": "j"},
            "hop_count": 0,
        }


# ---------------------------------------------------------------------------
# The bridge: publishing
# ---------------------------------------------------------------------------


class TestPublishing:
    async def test_registrations_made_before_start_are_published_at_start(self, services, backend, events):
        record = services.register("weather.v1", "2.0", {"format": "json"})
        bridge = DnsSdBridge(services, on_event=events.append, backend=backend)
        await bridge.start(address=OUR_ADDRESS, port=7878, addresses=["192.168.1.9", "fe80::1"])
        try:
            name = service_instance_name(OUR_ADDRESS, "weather.v1")
            assert bridge.published() == ["weather.v1"]
            _, kw = backend.registered[name]
            assert kw == {
                "name": name,
                "port": 7878,
                "txt": txt_record(record, OUR_ADDRESS),
                "server": service_host_name(OUR_ADDRESS),
                "addresses": ["192.168.1.9", "fe80::1"],
            }
        finally:
            await bridge.stop()

    async def test_a_registration_made_while_running_is_published(self, bridge, services, backend):
        services.register("wiki.first-aid", "1.0")
        await settle()
        assert service_instance_name(OUR_ADDRESS, "wiki.first-aid") in backend.registered

    async def test_an_unregistration_withdraws(self, bridge, services, backend):
        services.register("wiki.first-aid")
        await settle()
        handle, _ = backend.registered[service_instance_name(OUR_ADDRESS, "wiki.first-aid")]
        services.unregister("wiki.first-aid")
        await settle()
        assert backend.registered == {}
        assert backend.unregistered == [handle]
        assert bridge.published() == []

    async def test_registering_again_replaces_the_instance(self, bridge, services, backend):
        services.register("wiki", "1.0")
        await settle()
        name = service_instance_name(OUR_ADDRESS, "wiki")
        first, _ = backend.registered[name]
        services.register("wiki", "2.0")
        await settle()
        second, kw = backend.registered[name]
        assert second is not first
        assert first in backend.unregistered
        assert kw["txt"]["ver"] == "2.0"
        assert bridge.published() == ["wiki"]

    async def test_publishing_the_same_id_twice_directly_keeps_one_instance(self, bridge, backend):
        # The listener path withdraws before it publishes; a direct caller
        # (start() over an id something already published) must not leave
        # two handles for one name.
        assert await bridge.publish(ServiceRecord("wiki", "1.0")) is True
        name = service_instance_name(OUR_ADDRESS, "wiki")
        first, _ = backend.registered[name]
        assert await bridge.publish(ServiceRecord("wiki", "2.0")) is True
        second, kw = backend.registered[name]
        assert second is not first
        assert backend.unregistered == [first]
        assert kw["txt"]["ver"] == "2.0"
        assert bridge.published() == ["wiki"]

    async def test_a_port_of_none_publishes_zero(self, services, backend):
        services.register("s")
        bridge = DnsSdBridge(services, backend=backend)
        await bridge.start(address=OUR_ADDRESS, addresses=["10.0.0.1"])
        try:
            _, kw = backend.registered[service_instance_name(OUR_ADDRESS, "s")]
            assert kw["port"] == 0
        finally:
            await bridge.stop()

    async def test_a_descriptor_that_does_not_fit_is_not_published_and_the_mesh_keeps_it(
        self, bridge, services, backend, mesh, caplog
    ):
        with caplog.at_level(logging.WARNING):
            services.register("a" * 201)
            await settle()
        assert backend.registered == {}
        assert bridge.published() == []
        assert services.registered()[0].service_id == "a" * 201
        mesh.register_service.assert_called_once()
        assert "not published on the LAN" in caplog.text and "201 bytes" in caplog.text

    async def test_publish_refuses_anything_but_this_nodes_own(self, bridge):
        # Invariant 2: a LAN claim never goes out under this node's identity.
        lan = ServiceRecord("s", provider=PEER_ADDRESS, source=SOURCE_LAN)
        with pytest.raises(ValueError, match="invariant 2"):
            await bridge.publish(lan)

    async def test_publish_before_start_does_nothing(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend)
        assert await bridge.publish(ServiceRecord("s")) is False
        assert backend.registered == {}

    async def test_a_listener_call_before_start_or_after_stop_is_dropped(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend)
        bridge.on_registered(ServiceRecord("s"))  # no loop yet: ignored
        await bridge.start(address=OUR_ADDRESS, addresses=["10.0.0.1"])
        await bridge.stop()
        bridge.on_registered(ServiceRecord("s"))
        await settle()
        assert backend.registered == {}

    async def test_browse_only_publishes_nothing(self, services, backend):
        services.register("s")
        bridge = DnsSdBridge(services, backend=backend, publish=False)
        await bridge.start(address=OUR_ADDRESS)
        try:
            assert backend.registered == {}
            assert backend.on_change is not None
        finally:
            await bridge.stop()

    async def test_publish_only_does_not_browse(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend, browse=False)
        await bridge.start(address=OUR_ADDRESS, addresses=["10.0.0.1"])
        try:
            assert backend.on_change is None
        finally:
            await bridge.stop()


# ---------------------------------------------------------------------------
# The bridge: importing
# ---------------------------------------------------------------------------


def peer_txt(**overrides: Any) -> bytes:
    entries = {"txtvers": "1", "sid": "weather.v1", "ver": "2.0", "addr": PEER_ADDRESS, "c.format": "json"}
    entries.update(overrides)
    return encode_txt(entries)


PEER_NAME = service_instance_name(PEER_ADDRESS, "weather.v1")


class TestImporting:
    async def test_a_browsed_instance_is_resolved_imported_and_announced(self, bridge, backend, events):
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.on_change(PEER_NAME, False)
        await settle()
        assert backend.resolve_calls == [PEER_NAME]
        assert events == [
            {
                "type": "service_discovered",
                "source": "lan",
                "query_id": "",
                "service_id": "weather.v1",
                "version": "2.0",
                "provider_peer_id": PEER_ADDRESS,
                "capabilities": {"format": "json"},
                "hop_count": 0,
            }
        ]
        assert bridge.lan_services() == [
            ServiceRecord("weather.v1", "2.0", {"format": "json"}, provider=PEER_ADDRESS, source=SOURCE_LAN)
        ]

    async def test_an_import_never_reaches_the_engine(self, bridge, backend, mesh):
        # Invariant 2, at the seam that matters: nothing on the import path
        # calls the engine, whatever the record says.
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.on_change(PEER_NAME, False)
        await settle()
        assert bridge.lan_services() != []
        mesh.register_service.assert_not_called()
        mesh.unregister_service.assert_not_called()

    async def test_our_own_record_is_ignored(self, bridge, backend, events):
        name = service_instance_name(OUR_ADDRESS, "weather.v1")
        backend.txt_by_name[name] = peer_txt(addr=OUR_ADDRESS)
        backend.on_change(name, False)
        await settle()
        assert events == [] and bridge.lan_services() == []

    async def test_an_unresolvable_instance_is_not_imported(self, bridge, backend, events):
        backend.on_change(PEER_NAME, False)
        await settle()
        assert events == [] and bridge.lan_services() == []

    async def test_a_record_the_chapter_ignores_removes_an_earlier_import(self, bridge, backend, events):
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.on_change(PEER_NAME, False)
        await settle()
        assert len(bridge.lan_services()) == 1
        backend.txt_by_name[PEER_NAME] = peer_txt(txtvers="2")
        backend.on_change(PEER_NAME, False)
        await settle()
        assert bridge.lan_services() == []
        assert len(events) == 1

    async def test_a_changed_record_is_announced_again_and_an_unchanged_one_is_not(self, bridge, backend, events):
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.on_change(PEER_NAME, False)
        await settle()
        backend.on_change(PEER_NAME, False)  # Updated with the same content
        await settle()
        assert len(events) == 1
        backend.txt_by_name[PEER_NAME] = peer_txt(ver="3.0")
        backend.on_change(PEER_NAME, False)
        await settle()
        assert len(events) == 2 and events[1]["version"] == "3.0"
        assert [r.version for r in bridge.lan_services()] == ["3.0"]

    async def test_a_removed_instance_leaves_the_registry(self, bridge, backend):
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.on_change(PEER_NAME, False)
        await settle()
        backend.on_change(PEER_NAME, True)
        assert bridge.lan_services() == []

    async def test_two_services_from_one_neighbour_are_two_imports(self, bridge, backend):
        other = service_instance_name(PEER_ADDRESS, "wiki")
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.txt_by_name[other] = peer_txt(sid="wiki")
        backend.on_change(PEER_NAME, False)
        backend.on_change(other, False)
        await settle()
        assert sorted(r.service_id for r in bridge.lan_services()) == ["weather.v1", "wiki"]

    async def test_a_failing_event_handler_is_contained(self, services, backend, caplog):
        handler = MagicMock(side_effect=RuntimeError("boom"))
        bridge = DnsSdBridge(services, on_event=handler, backend=backend, publish=False)
        await bridge.start(address=OUR_ADDRESS)
        try:
            backend.txt_by_name[PEER_NAME] = peer_txt()
            backend.on_change(PEER_NAME, False)
            await settle()
            assert len(bridge.lan_services()) == 1
            assert "event handler failed" in caplog.text
        finally:
            await bridge.stop()


class TestLifetime:
    async def test_an_import_is_re_resolved_at_half_the_ttl_and_dropped_at_the_ttl(self, bridge, backend):
        backend.txt_by_name[PEER_NAME] = peer_txt()
        bridge.import_txt(PEER_NAME, peer_txt(), now=1000.0)
        backend.resolve_calls.clear()

        await bridge.sweep(now=1000.0 + 49.0)  # under half of 100: untouched
        assert backend.resolve_calls == []
        await bridge.sweep(now=1000.0 + 50.0)  # half: re-resolved, and it answers
        assert backend.resolve_calls == [PEER_NAME]
        assert len(bridge.lan_services()) == 1

        del backend.txt_by_name[PEER_NAME]  # the neighbour is gone without a goodbye
        await bridge.sweep(now=1050.0 + 50.0)  # half again since the refresh: asked, no answer
        assert backend.resolve_calls == [PEER_NAME, PEER_NAME]
        assert len(bridge.lan_services()) == 1  # still within the ttl of its last answer
        await bridge.sweep(now=1050.0 + 99.0)
        assert len(bridge.lan_services()) == 1
        await bridge.sweep(now=1050.0 + 100.0)  # the ttl since the last answer
        assert bridge.lan_services() == []

    async def test_a_successful_re_resolution_refreshes_the_clock_silently(self, bridge, backend, events):
        bridge.import_txt(PEER_NAME, peer_txt(), now=0.0)
        backend.txt_by_name[PEER_NAME] = peer_txt()
        await bridge.sweep(now=60.0)
        await bridge.sweep(now=120.0)  # would have expired at 100 without the refresh at 60
        assert len(bridge.lan_services()) == 1
        assert len(events) == 1

    async def test_the_sweeper_runs_on_its_own(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend, publish=False, ttl=0.04)
        await bridge.start(address=OUR_ADDRESS)
        try:
            bridge.import_txt(PEER_NAME, peer_txt())
            await asyncio.sleep(0.15)
            assert bridge.lan_services() == []
            assert PEER_NAME in backend.resolve_calls
        finally:
            await bridge.stop()

    def test_a_ttl_must_be_positive(self, services, backend):
        with pytest.raises(ValueError):
            DnsSdBridge(services, backend=backend, ttl=0)


# ---------------------------------------------------------------------------
# The bridge: lifecycle
# ---------------------------------------------------------------------------


class TestLifecycle:
    async def test_stop_withdraws_everything_and_closes_the_responder(self, services, backend):
        services.register("a")
        services.register("b")
        bridge = DnsSdBridge(services, backend=backend)
        await bridge.start(address=OUR_ADDRESS, addresses=["10.0.0.1"])
        backend.txt_by_name[PEER_NAME] = peer_txt()
        backend.on_change(PEER_NAME, False)
        await settle()
        assert len(backend.registered) == 2 and len(bridge.lan_services()) == 1
        await bridge.stop()
        assert backend.registered == {} and len(backend.unregistered) == 2
        assert backend.closed and not bridge.running
        assert bridge.lan_services() == [] and bridge.published() == []
        await bridge.stop()  # idempotent

    async def test_start_is_idempotent(self, bridge, backend):
        await bridge.start(address=PEER_ADDRESS, addresses=["10.0.0.2"])
        assert bridge.address == OUR_ADDRESS

    async def test_start_refuses_a_non_address(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend)
        with pytest.raises(ValueError, match="canonical address"):
            await bridge.start(address="alice", addresses=["10.0.0.1"])
        assert not bridge.running

    async def test_start_with_nothing_to_publish_from_fails_and_closes(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend)
        with pytest.raises(RuntimeError, match="no interface address"):
            await bridge.start(address=OUR_ADDRESS, addresses=[])
        assert backend.closed and not bridge.running

    async def test_a_responder_refusal_at_start_unwinds(self, services, backend):
        # A name conflict or a closed socket at the first publish: the bridge
        # must not stay half-started, listening to the registry with an open
        # responder while reporting itself stopped.
        services.register("a")

        async def refuse(**kw: Any) -> object:
            raise RuntimeError("name conflict")

        backend.register = refuse  # type: ignore[assignment]
        bridge = DnsSdBridge(services, backend=backend)
        with pytest.raises(RuntimeError, match="name conflict"):
            await bridge.start(address=OUR_ADDRESS, addresses=["10.0.0.1"])
        assert not bridge.running
        assert bridge not in services._listeners
        assert backend.closed
        assert backend.on_change is None

    async def test_the_listener_is_removed_at_stop(self, services, backend):
        bridge = DnsSdBridge(services, backend=backend)
        await bridge.start(address=OUR_ADDRESS, addresses=["10.0.0.1"])
        assert bridge in services._listeners
        await bridge.stop()
        assert bridge not in services._listeners

    def test_the_real_responder_registers_under_the_subtype(self):
        # The one line of real zeroconf exercised here: the constructor's
        # name check accepts an instance under the base type registered
        # with the subtype, and refuses a name that is not under the type.
        zeroconf = pytest.importorskip("zeroconf")
        info = zeroconf.ServiceInfo(
            SUBTYPE,
            service_instance_name(OUR_ADDRESS, "s"),
            port=0,
            properties=txt_record(ServiceRecord("s"), OUR_ADDRESS),
            server=service_host_name(OUR_ADDRESS),
            parsed_addresses=["10.0.0.1"],
        )
        assert info.type == SUBTYPE
        assert record_from_txt(bytes(info.text)) == ServiceRecord(
            "s", provider=OUR_ADDRESS, source=SOURCE_LAN
        )
        with pytest.raises(zeroconf.BadTypeInNameException):
            zeroconf.ServiceInfo(SUBTYPE, "s._other._tcp.local.", port=0)

    def test_the_real_responder_is_built_lazily(self):
        pytest.importorskip("zeroconf")
        backend = ZeroconfBackend()
        assert backend is not None
        asyncio.run(backend.close())
