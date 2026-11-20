"""``app_id`` keeps working as the deprecated name of ``app_label``."""

import warnings

import pytest

from offline_protocol_sdk import MediaSendOptions, OverflowPolicy, ProtocolConfig, SendMessageOptions


def _config(**name):
    return ProtocolConfig(
        **name,
        profile="default",
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=False,
        initial_ttl=8,
        encryption_enabled=False,
        auto_key_exchange=False,
        store_pending=True,
        max_pending_per_peer=10,
        max_pending_global=100,
        pending_ttl_ms=60_000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


def test_the_old_keyword_sets_the_label_and_warns():
    with pytest.warns(DeprecationWarning, match="appLabel|app_label"):
        config = _config(app_id="app.muster")
    assert config.app_label == "app.muster"
    with pytest.warns(DeprecationWarning):
        assert config.app_id == "app.muster"


def test_the_new_keyword_does_not_warn():
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        assert _config(app_label="app.muster").app_label == "app.muster"


def test_both_names_must_agree():
    with pytest.warns(DeprecationWarning), pytest.raises(TypeError, match="disagree"):
        _config(app_label="one", app_id="two")


@pytest.mark.parametrize("cls", [SendMessageOptions, MediaSendOptions])
def test_send_options_take_the_old_keyword_too(cls):
    with pytest.warns(DeprecationWarning):
        options = cls(app_id="other-app")
    assert options.app_label == "other-app"
