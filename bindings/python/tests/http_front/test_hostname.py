from __future__ import annotations

import json

import pytest

from offline_protocol_sdk.http_front.hostname import DEFAULT_DOMAIN, Aliases, HostError, Target, parse_host

ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"


def test_the_default_domain_is_the_chapters():
    assert DEFAULT_DOMAIN == "offline.protocol.internal"


@pytest.mark.parametrize(
    "host, expected",
    [
        (f"timeofday.{ADDRESS}.offline.protocol.internal", Target("timeofday", ADDRESS)),
        ("TimeOfDay.Bob.Offline.Protocol.Internal:8080", Target("timeofday", "bob")),
        ("timeofday.bob.offline.protocol.internal.", Target("timeofday", "bob")),
        ("offline.protocol.internal", None),
        ("offline.protocol.internal:8080", None),
        ("127.0.0.1:8080", None),
        ("[::1]:8080", None),
        ("localhost", None),
        (None, None),
        ("", None),
    ],
)
def test_a_host_names_a_target_or_the_front_itself(host, expected):
    assert parse_host(host) == expected


@pytest.mark.parametrize(
    "host",
    [
        "bob.offline.protocol.internal",
        "a.b.c.offline.protocol.internal",
        "-bad.bob.offline.protocol.internal",
        "time_of_day.bob.offline.protocol.internal",
        "timeofday..offline.protocol.internal",
    ],
)
def test_a_host_under_the_domain_that_is_not_service_dot_device_is_refused(host):
    with pytest.raises(HostError):
        parse_host(host)


def test_another_domain_is_honoured():
    assert parse_host("svc.bob.mesh.example", "mesh.example") == Target("svc", "bob")
    assert parse_host("svc.bob.offline.protocol.internal", "mesh.example") is None


def test_an_alias_resolves_and_an_address_resolves_to_itself(tmp_path):
    path = tmp_path / "aliases.json"
    path.write_text(json.dumps({"aliases": {"bob": ADDRESS}}))
    aliases = Aliases.load(path)
    assert aliases.resolve("bob") == ADDRESS
    assert aliases.resolve(ADDRESS) == ADDRESS
    assert aliases.resolve("carol") is None
    assert aliases.alias_of(ADDRESS) == "bob"


@pytest.mark.parametrize(
    "doc",
    [
        {"aliases": {ADDRESS: ADDRESS}},
        {"aliases": {"bob": "off1notanaddress"}},
        {"aliases": {"Bob_1": ADDRESS}},
        {"aliases": {"bob": ADDRESS}, "extra": 1},
        ["bob"],
    ],
)
def test_an_alias_file_that_could_mislead_is_refused(tmp_path, doc):
    """An alias shaped like an address would shadow a real one."""
    path = tmp_path / "aliases.json"
    path.write_text(json.dumps(doc))
    with pytest.raises(ValueError):
        Aliases.load(path)
