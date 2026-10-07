"""``--http`` on the service command and the standalone front command."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from local_api.test_local_api_server import _cli_args


def test_http_is_off_unless_asked(tmp_path):
    cli, args = _cli_args(tmp_path)
    assert cli.http_front_options(args) is None


def test_http_options_keep_registrations_beside_the_state_root(tmp_path):
    cli, args = _cli_args(tmp_path, "--http", "127.0.0.1:8080")
    options = cli.http_front_options(args)
    assert (options["host"], options["port"]) == ("127.0.0.1", 8080)
    assert options["registry_path"] == str(Path(tmp_path / "state") / "http-front-services.json")
    assert options["domain"] == "offline.protocol.internal"


def test_http_off_loopback_needs_a_token_file(tmp_path):
    cli, args = _cli_args(tmp_path, "--http", "0.0.0.0:8080")
    with pytest.raises(SystemExit, match="--http-token-file"):
        cli.http_front_options(args)
    cli, args = _cli_args(tmp_path, "--http", "0.0.0.0:8080", "--http-token-file", str(tmp_path / "t"))
    assert cli.http_front_options(args)["token_path"] == str(tmp_path / "t")


def test_a_bad_alias_file_stops_the_command_before_anything_starts(tmp_path):
    aliases = tmp_path / "aliases.json"
    aliases.write_text(json.dumps({"aliases": {"bob": "not-an-address"}}))
    cli, args = _cli_args(tmp_path, "--http", "127.0.0.1:8080", "--http-aliases", str(aliases))
    with pytest.raises(SystemExit, match="--http-aliases"):
        cli.http_front_options(args)


def test_a_malformed_listen_address_is_refused(tmp_path):
    cli, args = _cli_args(tmp_path, "--http", "8080")
    with pytest.raises(SystemExit, match="HOST:PORT"):
        cli.http_front_options(args)


def test_the_standalone_command_needs_the_service_token_with_tcp():
    from offline_protocol_sdk.http_front import cli

    args = cli.build_parser().parse_args(["--tcp", "7878", "--listen", "127.0.0.1:8080"])
    with pytest.raises(SystemExit, match="--token-file"):
        cli.build_front(args)


def test_the_standalone_command_builds_a_front_on_a_socket(tmp_path):
    from offline_protocol_sdk.http_front import cli

    args = cli.build_parser().parse_args(
        ["--socket", str(tmp_path / "api.sock"), "--listen", "127.0.0.1:0", "--http-domain", "mesh.example"]
    )
    front = cli.build_front(args)
    assert front.domain == "mesh.example"
