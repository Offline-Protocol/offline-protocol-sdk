package com.offlineprotocol.sdk;


final class NamespaceLibrary {
    static synchronized String findLibraryName(String componentName) {
        String libOverride = System.getProperty("uniffi.component." + componentName + ".libraryOverride");
        if (libOverride != null) {
            return libOverride;
        }
        return "offline_protocol_uniffi";
    }

    static java.lang.foreign.SymbolLookup loadLibrary() {
        String name = findLibraryName("offline_protocol");
        if (name.startsWith("/") // Unix absolute path
                || name.startsWith("\\\\") // Windows UNC path
                || (name.length() > 2 && name.charAt(1) == ':')) // Windows drive path (e.g. C:\)
        {
            System.load(name);
        } else {
            System.loadLibrary(name);
        }
        return java.lang.foreign.SymbolLookup.loaderLookup();
    }

    static void uniffiCheckContractApiVersion() {
        int bindingsContractVersion = 30;
        int scaffoldingContractVersion = UniffiLib.ffi_offline_protocol_uniffi_uniffi_contract_version();
        if (bindingsContractVersion != scaffoldingContractVersion) {
            throw new RuntimeException("UniFFI contract version mismatch: try cleaning and rebuilding your project");
        }
    }
    static void uniffiCheckApiChecksums() {
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_func_derive_address() != ((short) 64789)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_func_parse_invite() != ((short) 15865)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_func_run_storage_conformance() != ((short) 20699)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_func_verify_identity_assertion() != ((short) 61480)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_attachment_hash() != ((short) 10852)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_increment() != ((short) 50001)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_counter_value() != ((short) 19947)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_create_doc() != ((short) 43277)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_decline_attachment() != ((short) 53249)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_delete_doc() != ((short) 23496)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_json() != ((short) 62388)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_doc_size() != ((short) 1306)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_export_raw() != ((short) 30993)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment() != ((short) 2774)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_fetch_attachment_from() != ((short) 44676)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_flush() != ((short) 4911)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_flush_all() != ((short) 58838)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_list_delete() != ((short) 46614)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_list_docs() != ((short) 43688)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_list_len() != ((short) 53366)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_list_push() != ((short) 58133)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_list_spaces() != ((short) 25003)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_map_delete() != ((short) 61779)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_map_get_json() != ((short) 7048)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_map_set() != ((short) 60305)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_provide_attachment() != ((short) 57819)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_doc() != ((short) 58710)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_remove_space() != ((short) 242)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_set_interest() != ((short) 5786)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_text_delete() != ((short) 12470)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_text_insert() != ((short) 42508)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_text_value() != ((short) 6495)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_datastore_wipe_all() != ((short) 58624)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_meshservices_discover_services() != ((short) 43941)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_meshservices_register_service() != ((short) 46597)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_meshservices_respond_to_service_request() != ((short) 20926)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_meshservices_send_service_request() != ((short) 34689)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_meshservices_unregister_service() != ((short) 4997)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_accept_connection_request() != ((short) 50880)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_clear_peer_mtu() != ((short) 19884)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_fallback_count() != ((short) 60784)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_fragment_received() != ((short) 9764)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_next_fragment() != ((short) 30344)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_get_peer_count() != ((short) 11879)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_node_hash() != ((short) 1579)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_discovered() != ((short) 26789)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_lost() != ((short) 22804)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_peer_rssi() != ((short) 9983)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_recipient_not_among_peers_count() != ((short) 42065)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_return_fragment() != ((short) 51260)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_set_peer_mtu() != ((short) 7996)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_status_changed() != ((short) 63523)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_ble_undersized_mtu_reports() != ((short) 53668)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_block_user() != ((short) 27957)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_connection_request() != ((short) 17522)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_cancel_file_transfer() != ((short) 44634)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_close_file_stores() != ((short) 57369)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_group() != ((short) 13199)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_create_invite() != ((short) 21745)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_derive_user_id_from_public_key() != ((short) 46701)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_disable_telemetry() != ((short) 12013)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_emit_test_event() != ((short) 29503)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_enable_telemetry() != ((short) 52940)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_end_telemetry_session() != ((short) 55610)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_erase_custody() != ((short) 39575)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_establish_secure_session() != ((short) 52793)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_finalize_file() != ((short) 48884)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry() != ((short) 52440)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_flush_telemetry_blocking() != ((short) 60745)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_force_transport() != ((short) 65079)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message() != ((short) 4746)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_forward_message_to_group() != ((short) 43987)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_gateway_address_declaration() != ((short) 21061)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_active_transports() != ((short) 23761)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_battery_level() != ((short) 56170)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_blocked_users() != ((short) 28218)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_custody_stats() != ((short) 29466)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dedup_stats() != ((short) 51917)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_delivery_success_rate() != ((short) 23445)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_dors_config() != ((short) 10752)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_establishment_state() != ((short) 61148)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_file_progress() != ((short) 25115)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_info() != ((short) 11773)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_group_roles() != ((short) 19243)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_identity_public_key() != ((short) 48103)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_is_charging() != ((short) 42857)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_hops() != ((short) 54486)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_median_latency() != ((short) 20827)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_member_role() != ((short) 5518)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_stats() != ((short) 29444)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_mesh_relay_tunables() != ((short) 34919)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_message_stats() != ((short) 26794)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_pending_ack_count() != ((short) 14526)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_config() != ((short) 41352)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_relay_priority() != ((short) 56317)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_retry_queue_size() != ((short) 11855)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_state() != ((short) 56454)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_topology() != ((short) 14852)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_get_transport_metrics() != ((short) 47103)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_relay_sync_state() != ((short) 2982)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_group_rich_readiness() != ((short) 33044)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_has_pending_key_package() != ((short) 50833)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_identity_assertion() != ((short) 12802)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls() != ((short) 28255)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_initialize_mls_with_file_stores() != ((short) 54918)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declaration_refused() != ((short) 52855)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_address_declared() != ((short) 5462)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_confirm_sent() != ((short) 22824)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_get_next_message() != ((short) 42018)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_group_report_received() != ((short) 12840)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_message_received() != ((short) 4763)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_peer_presence() != ((short) 6161)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_presence_watchlist() != ((short) 9479)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_relay_capabilities() != ((short) 379)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed() != ((short) 12455)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_send_failed_with_reason() != ((short) 50456)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_internet_status_changed() != ((short) 58507)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_invite_to_group() != ((short) 38704)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_mls_initialized() != ((short) 58599)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_relay() != ((short) 22217)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_is_user_blocked() != ((short) 9262)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_leave_group() != ((short) 13144)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_list_groups() != ((short) 26113)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_local_address() != ((short) 54626)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_clear_pending_welcome() != ((short) 33145)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_create_session() != ((short) 13710)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt() != ((short) 22215)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_decrypt_from_user() != ((short) 29387)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_delete_session() != ((short) 11647)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_encrypt_for_user() != ((short) 1970)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_generate_key_package() != ((short) 34344)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_or_create_key_package() != ((short) 26823)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_key_packages() != ((short) 3234)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_get_pending_welcome() != ((short) 39261)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_has_session() != ((short) 56067)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_import_key_package() != ((short) 44533)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_join_session() != ((short) 14485)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_list_sessions() != ((short) 23429)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_mark_key_package_synced() != ((short) 6355)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_mls_process_welcome() != ((short) 12812)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_confirm_sent() != ((short) 9310)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_message() != ((short) 20908)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_next_query() != ((short) 23327)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_public_key() != ((short) 59792)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_get_subscription_filter() != ((short) 35803)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received() != ((short) 32701)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_message_received_at() != ((short) 4372)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_completed() != ((short) 45714)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_query_event_received() != ((short) 35705)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed() != ((short) 34587)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_send_failed_with_reason() != ((short) 46967)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_nostr_status_changed() != ((short) 10042)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_notify_app_state() != ((short) 64828)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_pause() != ((short) 2754)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_poll_event() != ((short) 2150)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process() != ((short) 28590)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_process_file_chunk() != ((short) 38467)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_protocol_lock_diagnostics() != ((short) 63421)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_receive_message() != ((short) 24572)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reject_connection_request() != ((short) 17619)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rekey_session() != ((short) 49857)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_release_transport_lock() != ((short) 9066)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_from_group() != ((short) 50435)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_remove_transport() != ((short) 37995)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_rename_group() != ((short) 36114)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_request_group_relay_registration() != ((short) 3982)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resolve_username() != ((short) 56081)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_resume() != ((short) 23596)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declaration_refused() != ((short) 6207)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_address_declared() != ((short) 14810)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_confirm_sent() != ((short) 49416)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_gateway_capabilities() != ((short) 38346)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_get_next_message() != ((short) 43573)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_message_received() != ((short) 6557)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_peer_presence() != ((short) 9523)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_presence_watchlist() != ((short) 26815)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed() != ((short) 54693)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_send_failed_with_reason() != ((short) 59249)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_reticulum_status_changed() != ((short) 14680)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_connection_request() != ((short) 32445)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file() != ((short) 51826)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_file_from_path() != ((short) 60049)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_group_message() != ((short) 11928)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media() != ((short) 26391)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_media_rich() != ((short) 17024)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message() != ((short) 58608)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_message_rich() != ((short) 3404)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_presence_update() != ((short) 56108)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_read_receipt() != ((short) 4730)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_send_typing_indicator() != ((short) 7836)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_level() != ((short) 44179)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_battery_state() != ((short) 59397)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_ble_transport_callback() != ((short) 15468)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_event_callback() != ((short) 57350)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_member_role() != ((short) 9132)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_nostr_transport_callback() != ((short) 36253)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_peer_rssi_events() != ((short) 39361)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_relay_priority() != ((short) 26597)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_reticulum_transport_callback() != ((short) 53618)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_telemetry_enabled() != ((short) 65413)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_set_wifi_direct_transport_callback() != ((short) 33195)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_should_escalate_to_wifi() != ((short) 36523)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data() != ((short) 58338)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_sign_data_bytes() != ((short) 21454)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_start() != ((short) 63301)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_stop() != ((short) 30235)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_install_id() != ((short) 43250)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_telemetry_stats() != ((short) 57652)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_unblock_user() != ((short) 52650)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_ack_config() != ((short) 50568)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dedup_config() != ((short) 37188)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_dors_config() != ((short) 42995)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_relay_config() != ((short) 33211)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_retry_config() != ((short) 3523)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_update_transport_metrics() != ((short) 15902)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature() != ((short) 16718)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_verify_signature_bytes() != ((short) 24438)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_get_next_message() != ((short) 50868)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_message_received() != ((short) 59107)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_connected() != ((short) 38596)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_peer_disconnected() != ((short) 15986)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_offlineprotocol_wifi_direct_status_changed() != ((short) 12337)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_constructor_datastore_new() != ((short) 36671)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_constructor_datastore_with_storage() != ((short) 2283)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_constructor_meshservices_new() != ((short) 61363)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_constructor_offlineprotocol_new() != ((short) 30125)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_bletransportcallback_on_fragments_available() != ((short) 43182)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_eventcallback_on_event() != ((short) 15485)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_store() != ((short) 9534)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_load() != ((short) 33472)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_delete() != ((short) 40050)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_mlsstorageprovider_list_keys() != ((short) 29193)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_nostrtransportcallback_on_messages_available() != ((short) 6903)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_store() != ((short) 40025)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_load() != ((short) 12081)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_delete() != ((short) 51503)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_protocolstatestorageprovider_list_keys() != ((short) 5874)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_reticulumtransportcallback_on_messages_available() != ((short) 26982)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
        if (UniffiLib.uniffi_offline_protocol_uniffi_checksum_method_wifidirecttransportcallback_on_messages_available() != ((short) 16192)) {
            throw new RuntimeException("UniFFI API checksum mismatch: try cleaning and rebuilding your project");
        }
    }
}

// Define FFI callback types
