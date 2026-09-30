"""The method table: which declarations are on the wire, and the call path
from a JSON-RPC request to the generated binding and back.

``EXPOSED`` and ``PLATFORM`` classify every declaration in ``table.py`` by
its wire name. The two sets are the chapter's method table and platform
table, and a Rust guard in the FFI crate holds all three to each other: a
declaration in neither set fails that guard and the test in
``tests/local_api``, so an unclassified method is a failing test rather than
a method every local application can reach by default.

A call is decoded by the declared parameter types, checked against the
server-side rules, executed against the binding, and encoded by the declared
result type. Two methods are stamped: ``send_message`` and ``send_media``
are executed through their rich twins with the connection's application id
set, and the rich twins refuse an ``app_id`` the client sent.
"""

from __future__ import annotations

import asyncio
from typing import Any, Callable

from .. import offline_protocol as generated
from . import codec
from .authz import Policy, ServiceOwnership
from .codec import RpcError, error_from_protocol, taxonomy_error
from .mux import EventRouter
from .session import Session
from .table import TABLE

#: The interface definition's declarations a client may call, by wire name.
EXPOSED: frozenset[str] = frozenset(
    {
        # identity and state
        "get_state",
        "local_address",
        "get_identity_public_key",
        "derive_user_id_from_public_key",
        "sign_data",
        "verify_signature",
        "create_invite",
        "resolve_username",
        # messaging
        "send_message",
        "send_message_rich",
        "forward_message",
        "send_presence_update",
        "send_typing_indicator",
        "send_read_receipt",
        # connection requests
        "send_connection_request",
        "accept_connection_request",
        "reject_connection_request",
        "cancel_connection_request",
        # media and files
        "send_media",
        "send_media_rich",
        "send_file",
        "get_file_progress",
        "cancel_file_transfer",
        # secure sessions
        "is_mls_initialized",
        "has_pending_key_package",
        "get_establishment_state",
        "establish_secure_session",
        "rekey_session",
        "mls_has_session",
        "mls_list_sessions",
        "mls_delete_session",
        # manual MLS
        "mls_generate_key_package",
        "mls_get_or_create_key_package",
        "mls_import_key_package",
        "mls_get_pending_key_packages",
        "mls_mark_key_package_synced",
        "mls_create_session",
        "mls_join_session",
        "mls_encrypt_for_user",
        "mls_decrypt_from_user",
        "mls_get_pending_welcome",
        "mls_clear_pending_welcome",
        "mls_decrypt",
        "mls_process_welcome",
        # groups
        "create_group",
        "send_group_message",
        "forward_message_to_group",
        "invite_to_group",
        "remove_from_group",
        "leave_group",
        "list_groups",
        "get_group_info",
        "group_rich_readiness",
        "group_relay_sync_state",
        "request_group_relay_registration",
        "set_member_role",
        "get_member_role",
        "get_group_roles",
        "rename_group",
        # blocking
        "block_user",
        "unblock_user",
        "get_blocked_users",
        "is_user_blocked",
        # transports and metrics
        "get_active_transports",
        "get_transport_metrics",
        "should_escalate_to_wifi",
        "get_topology",
        "get_message_stats",
        "get_delivery_success_rate",
        "get_median_latency",
        "get_median_hops",
        "get_battery_level",
        "get_is_charging",
        "is_relay",
        "get_relay_priority",
        "get_relay_config",
        "get_dors_config",
        "get_dedup_stats",
        "get_pending_ack_count",
        "get_retry_queue_size",
        "get_mesh_relay_stats",
        "get_mesh_relay_tunables",
        # instance-wide tuning
        "set_relay_priority",
        "update_relay_config",
        "force_transport",
        "release_transport_lock",
        "update_dors_config",
        "update_ack_config",
        "update_retry_config",
        "update_dedup_config",
        # services
        "services.register_service",
        "services.unregister_service",
        "services.discover_services",
        "services.send_service_request",
        "services.respond_to_service_request",
        # documents
        "data.create_doc",
        "data.delete_doc",
        "data.remove_doc",
        "data.remove_space",
        "data.set_interest",
        "data.list_docs",
        "data.list_spaces",
        "data.map_set",
        "data.map_delete",
        "data.map_get_json",
        "data.list_push",
        "data.list_delete",
        "data.list_len",
        "data.text_insert",
        "data.text_delete",
        "data.text_value",
        "data.counter_increment",
        "data.counter_value",
        "data.doc_json",
        "data.export_raw",
        "data.flush",
        "data.flush_all",
        "data.doc_size",
        "data.attachment_hash",
        "data.fetch_attachment",
        "data.provide_attachment",
        "data.decline_attachment",
        "data.fetch_attachment_from",
        # instance-less
        "derive_address",
        "parse_invite",
        "verify_identity_assertion",
    }
)

#: The declarations the server owns and never puts on the wire (the
#: chapter's fifth invariant). A request naming one is answered with
#: method-not-found, the same answer an unknown name gets.
PLATFORM: frozenset[str] = frozenset(
    {
        # lifecycle, run loop, drain
        "constructor",
        "start",
        "stop",
        "pause",
        "resume",
        "process",
        "receive_message",
        "set_event_callback",
        "poll_event",
        "emit_test_event",
        # storage attach
        "initialize_mls",
        "initialize_mls_with_file_stores",
        "close_file_stores",
        # telemetry (C12)
        "enable_telemetry",
        "disable_telemetry",
        "set_telemetry_enabled",
        "flush_telemetry",
        "flush_telemetry_blocking",
        "telemetry_stats",
        "end_telemetry_session",
        "notify_app_state",
        "telemetry_install_id",
        # callback installers
        "set_ble_transport_callback",
        "set_wifi_direct_transport_callback",
        "set_reticulum_transport_callback",
        "set_nostr_transport_callback",
        # Bluetooth LE driver
        "ble_peer_discovered",
        "ble_peer_lost",
        "ble_status_changed",
        "ble_fragment_received",
        "ble_get_next_fragment",
        "ble_return_fragment",
        "ble_get_peer_count",
        "ble_set_peer_mtu",
        "ble_clear_peer_mtu",
        "ble_undersized_mtu_reports",
        "ble_fragment_fallback_count",
        "ble_recipient_not_among_peers_count",
        "protocol_lock_diagnostics",
        # relay client
        "internet_status_changed",
        "internet_message_received",
        "internet_get_next_message",
        "internet_confirm_sent",
        "internet_send_failed",
        "internet_send_failed_with_reason",
        "internet_peer_presence",
        "internet_presence_watchlist",
        "internet_relay_capabilities",
        "internet_group_report_received",
        "internet_address_declared",
        "internet_address_declaration_refused",
        # peer-stream driver
        "wifi_direct_status_changed",
        "wifi_direct_message_received",
        "wifi_direct_get_next_message",
        "wifi_direct_peer_connected",
        "wifi_direct_peer_disconnected",
        # gateway client
        "reticulum_status_changed",
        "reticulum_message_received",
        "reticulum_get_next_message",
        "reticulum_confirm_sent",
        "reticulum_send_failed",
        "reticulum_send_failed_with_reason",
        "reticulum_address_declared",
        "reticulum_address_declaration_refused",
        "reticulum_gateway_capabilities",
        "reticulum_peer_presence",
        "reticulum_presence_watchlist",
        # Nostr client
        "nostr_status_changed",
        "nostr_message_received",
        "nostr_message_received_at",
        "nostr_get_next_message",
        "nostr_confirm_sent",
        "nostr_send_failed",
        "nostr_send_failed_with_reason",
        "nostr_get_public_key",
        "nostr_get_subscription_filter",
        "nostr_get_next_query",
        "nostr_query_event_received",
        "nostr_query_completed",
        # host facts
        "update_transport_metrics",
        "remove_transport",
        "set_battery_level",
        "set_battery_state",
        # carrier proofs
        "identity_assertion",
        "gateway_address_declaration",
        # inbound chunk driver
        "process_file_chunk",
        "finalize_file",
        # the two objects' construction
        "services.constructor",
        "data.constructor",
        "data.with_storage",
        # the operator's logout
        "data.wipe_all",
        # takes a callback interface
        "run_storage_conformance",
    }
)

#: The server's own methods. Not in the definition; the only such methods.
SESSION_METHODS: frozenset[str] = frozenset({"hello", "subscribe", "unsubscribe"})

#: The one group a client may call before ``hello``: nothing to stamp,
#: nothing to route.
BEFORE_HELLO: frozenset[str] = frozenset(
    {"derive_address", "parse_invite", "verify_identity_assertion"}
)

#: Methods whose result is an identifier (or a list of them) that later
#: events name; the router records it as the caller's.
ID_RESULTS: frozenset[str] = frozenset(
    {
        "send_message",
        "send_message_rich",
        "forward_message",
        "send_presence_update",
        "send_typing_indicator",
        "send_read_receipt",
        "send_connection_request",
        "accept_connection_request",
        "reject_connection_request",
        "cancel_connection_request",
        "send_media",
        "send_media_rich",
        "send_file",
        "send_group_message",
        "forward_message_to_group",
        "services.discover_services",
        "services.send_service_request",
        "services.respond_to_service_request",
    }
)


def resolve(method: str) -> tuple[str, str]:
    """``(object, declaration)`` for a wire name."""
    if method.startswith("services."):
        return "MeshServices", method[len("services.") :]
    if method.startswith("data."):
        return "DataStore", method[len("data.") :]
    if method in TABLE["methods"]["namespace"]:
        return "namespace", method
    return "OfflineProtocol", method


def all_wire_names() -> frozenset[str]:
    """Every declaration in the table by its wire name."""
    prefixes = {"OfflineProtocol": "", "MeshServices": "services.", "DataStore": "data.", "namespace": ""}
    return frozenset(
        prefix + name for obj, prefix in prefixes.items() for name in TABLE["methods"][obj]
    )


class Dispatcher:
    """Executes exposed methods against the binding for one server."""

    def __init__(
        self,
        engine: Any,
        services: Any,
        data: Any,
        router: EventRouter,
        policy: Policy,
        ownership: ServiceOwnership,
        lock: asyncio.Lock,
    ) -> None:
        #: One lock for the whole server: engine calls run one at a time, on
        #: the default executor, so a slow one (a media send marshals its
        #: bytes per element in pure Python, about 1.5 s per MiB) neither
        #: stalls the event loop nor interleaves with another client's call,
        #: which is what keeps the caller rule's attribution exact.
        self._lock = lock
        self._engine = engine
        self._services = services
        #: The ``DataStore``, or the exception its construction raised, so
        #: every ``data.*`` call answers with the engine's own refusal.
        self._data = data
        self._router = router
        self._policy = policy
        self._ownership = ownership

    # -- entry ----------------------------------------------------------------

    async def call(self, session: Session, method: str, params: Any) -> Any:
        if method not in EXPOSED:
            raise RpcError(codec.METHOD_NOT_FOUND, f"unknown method {method}")
        if session.app_id is None:
            if method not in BEFORE_HELLO:
                raise taxonomy_error("InvalidState", "hello has not been sent on this connection")
        else:
            self._policy.check_method(session.app_id, method)
        obj, declaration = resolve(method)
        spec_params, result_type = TABLE["methods"][obj][declaration]
        args = self._decode_params(method, spec_params, params)
        target, declaration, args, result_type = self._prepare(
            session, method, obj, declaration, args, result_type
        )
        fn: Callable[..., Any] = getattr(target, declaration)
        loop = asyncio.get_running_loop()
        failure: RpcError | None = None
        result: Any = None
        try:
            async with self._lock:
                # Set while the lock is held: an event the engine emits on
                # the executor thread during this call reaches the loop
                # through `call_soon_threadsafe` ahead of the call's own
                # completion, so it is routed while this is still the caller.
                # An event the run loop emits meanwhile that names an
                # identifier nobody owns yet is parked by the router.
                self._router.current_caller = session
                try:
                    result = await loop.run_in_executor(None, lambda: fn(**args))
                except generated.ProtocolError as exc:
                    self._after_failure(method, args)
                    failure = error_from_protocol(exc)
                except RpcError as exc:
                    failure = exc
                except Exception as exc:  # the binding itself failed
                    self._after_failure(method, args)
                    failure = RpcError(codec.INTERNAL_ERROR, f"{method}: {exc}")
                finally:
                    self._router.current_caller = None
            if failure is None:
                result = self._after_success(session, method, args, result)
        finally:
            # Nothing has yielded since the caller was cleared, so the parked
            # events are routed with this call's identifiers recorded (or,
            # after a failure, with none issued), and before any other call
            # takes the lock.
            self._router.flush_parked()
        if failure is not None:
            raise failure
        return codec.encode(result_type, result)

    # -- parameters -----------------------------------------------------------

    @staticmethod
    def _decode_params(
        method: str, spec: tuple[tuple[str, str], ...], params: Any
    ) -> dict[str, Any]:
        if params is None:
            params = {}
        if not isinstance(params, dict):
            raise codec.invalid_params(f"{method}: params must be an object of named parameters")
        known = {name for name, _ in spec}
        unknown = sorted(set(params) - known)
        if unknown:
            raise codec.invalid_params(f"{method} has no parameter {unknown[0]!r}")
        args: dict[str, Any] = {}
        for name, type_name in spec:
            if name in params:
                args[name] = codec.decode(type_name, params[name], f"{method}.{name}")
            elif type_name.endswith("?"):
                args[name] = None
            else:
                raise codec.invalid_params(f"{method} requires {name}")
        return args

    # -- the rules and the stamping, before the call -------------------------

    def _prepare(
        self,
        session: Session,
        method: str,
        obj: str,
        declaration: str,
        args: dict[str, Any],
        result_type: str,
    ) -> tuple[Any, str, dict[str, Any], str]:
        app_id = session.app_id
        if obj == "namespace":
            return generated, declaration, args, result_type
        if obj == "MeshServices":
            assert app_id is not None
            service_id = args.get("service_id")
            if declaration == "register_service":
                self._ownership.claim(service_id, app_id)
            elif declaration in ("unregister_service", "respond_to_service_request"):
                self._ownership.check(service_id, app_id)
            return self._services, declaration, args, result_type
        if obj == "DataStore":
            assert app_id is not None
            if isinstance(self._data, BaseException):
                raise error_from_protocol(self._data)
            space_id = args.get("space_id")
            if isinstance(space_id, str):
                self._policy.check_space(app_id, space_id)
            return self._data, declaration, args, result_type
        # OfflineProtocol: the two stamped methods and their rich twins.
        assert app_id is not None
        if method == "send_message":
            options = generated.SendMessageOptions(
                priority=args["priority"], reply_to_msg=args["reply_to_msg"], app_id=app_id
            )
            return (
                self._engine,
                "send_message_rich",
                {"recipient": args["recipient"], "content": args["content"], "options": options},
                "string",
            )
        if method == "send_media":
            options = generated.MediaSendOptions(
                media_metadata=args["media_metadata"], app_id=app_id
            )
            return (
                self._engine,
                "send_media_rich",
                {
                    "recipient": args["recipient"],
                    "file_data": args["file_data"],
                    "file_name": args["file_name"],
                    "content_type": args["content_type"],
                    "options": options,
                },
                "string",
            )
        if method in ("send_message_rich", "send_media_rich"):
            options = args["options"]
            if options.app_id is not None:
                raise taxonomy_error(
                    "InvalidArgument",
                    "options.app_id is set by the server from the connection's hello",
                )
            options.app_id = app_id
        return self._engine, declaration, args, result_type

    # -- after the call -------------------------------------------------------

    def _after_failure(self, method: str, args: dict[str, Any]) -> None:
        if method == "services.register_service":
            # The engine refused the registration; a claim nothing backs
            # would block the next attempt by another application forever.
            self._ownership.release(args["service_id"])

    def _after_success(self, session: Session, method: str, args: dict[str, Any], result: Any) -> Any:
        app_id = session.app_id
        if method == "services.unregister_service" and result:
            self._ownership.release(args["service_id"])
        if method == "data.list_spaces" and app_id is not None:
            result = [space for space in result if self._policy.allows_space(app_id, space)]
        if method in ID_RESULTS and app_id is not None:
            self._router.note_ids(app_id, result if isinstance(result, list) else [result])
        return result
