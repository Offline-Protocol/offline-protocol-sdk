//! Unit tests for the replicated-document layer.

use crate::doc::DataDoc;
use crate::error::DataError;
use crate::policy;
use crate::value::DataValue;

fn doc_with_one_set() -> DataDoc {
    let mut doc = DataDoc::new();
    doc.map_set("profile", "name", DataValue::text("ada"))
        .expect("set");
    doc
}

#[test]
fn a_map_round_trips_through_a_commit() {
    let mut doc = doc_with_one_set();
    let delta = doc.commit().expect("commit").expect("a change");
    assert!(!delta.bytes.is_empty());
    assert_eq!(
        doc.map_get("profile", "name").expect("get"),
        Some(DataValue::text("ada"))
    );
}

#[test]
fn committing_twice_reports_no_second_change() {
    let mut doc = doc_with_one_set();
    doc.commit().expect("commit").expect("a change");
    assert!(doc.commit().expect("commit").is_none());
}

#[test]
fn a_delta_applied_to_a_second_replica_reproduces_the_state() {
    let mut source = doc_with_one_set();
    let delta = source.commit().expect("commit").expect("a change");

    let mut replica = DataDoc::new();
    replica.import(&delta.bytes).expect("import");

    assert_eq!(
        replica.map_get("profile", "name").expect("get"),
        Some(DataValue::text("ada"))
    );
}

#[test]
fn importing_the_same_delta_twice_is_a_no_op() {
    let mut source = DataDoc::new();
    source.list_push("log", DataValue::int(1)).expect("push");
    let delta = source.commit().expect("commit").expect("a change");

    let mut replica = DataDoc::new();
    replica.import(&delta.bytes).expect("first import");
    replica.import(&delta.bytes).expect("second import");

    // At-least-once delivery is the ladder's guarantee, so a duplicate has
    // to be free rather than merely tolerated.
    assert_eq!(replica.list_len("log").expect("len"), 1);
}

#[test]
fn deltas_applied_out_of_order_converge() {
    let mut source = DataDoc::new();
    source.text_insert("body", 0, "hello").expect("insert");
    let first = source.commit().expect("commit").expect("a change");
    source.text_insert("body", 5, " world").expect("insert");
    let second = source.commit().expect("commit").expect("a change");

    let mut replica = DataDoc::new();
    // Deliberately reversed: an unordered transport is exactly what this
    // layer is designed to ride.
    replica.import(&second.bytes).expect("import second");
    replica.import(&first.bytes).expect("import first");

    assert_eq!(replica.text_value("body").expect("text"), "hello world");
}

#[test]
fn concurrent_edits_on_two_replicas_merge_both_ways() {
    let mut left = DataDoc::new();
    left.map_set("settings", "theme", DataValue::text("dark"))
        .expect("set");
    let seed = left.commit().expect("commit").expect("a change");

    let mut right = DataDoc::new();
    right.import(&seed.bytes).expect("seed");

    left.map_set("settings", "font", DataValue::text("mono"))
        .expect("set");
    let from_left = left.commit().expect("commit").expect("a change");

    right
        .counter_increment("visits", 3.0)
        .expect("counter increment");
    let from_right = right.commit().expect("commit").expect("a change");

    left.import(&from_right.bytes).expect("import right");
    right.import(&from_left.bytes).expect("import left");

    for (label, replica) in [("left", &left), ("right", &right)] {
        assert_eq!(
            replica.map_get("settings", "theme").expect("get"),
            Some(DataValue::text("dark")),
            "{label} lost the seed"
        );
        assert_eq!(
            replica.map_get("settings", "font").expect("get"),
            Some(DataValue::text("mono")),
            "{label} lost the left edit"
        );
        assert_eq!(
            replica.counter_value("visits").expect("counter"),
            3.0,
            "{label} lost the right edit"
        );
    }
}

#[test]
fn a_compacted_snapshot_restores_the_document() {
    let mut source = DataDoc::new();
    for index in 0..200 {
        source
            .map_set("items", &format!("k{index}"), DataValue::int(index))
            .expect("set");
        if index % 50 == 0 {
            source.commit().expect("commit");
        }
    }
    source.commit().expect("commit");
    let compacted = source.export_compacted().expect("compact");

    let mut restored = DataDoc::new();
    restored.import(&compacted).expect("import compacted");
    assert_eq!(
        restored.map_get("items", "k199").expect("get"),
        Some(DataValue::int(199))
    );
}

#[test]
fn mark_persisted_stops_history_being_re_emitted() {
    let mut doc = doc_with_one_set();
    doc.commit().expect("commit").expect("a change");
    let compacted = doc.export_compacted().expect("compact");

    // Re-open the way the store does: load the record, then declare it
    // persisted. A commit right after must not hand back the history that
    // was just read off disk.
    let mut reopened = DataDoc::new();
    reopened.import(&compacted).expect("import");
    reopened.mark_persisted();
    assert!(reopened.commit().expect("commit").is_none());

    reopened
        .map_set("profile", "name", DataValue::text("grace"))
        .expect("set");
    let delta = reopened.commit().expect("commit").expect("a change");
    assert!(!delta.bytes.is_empty());
}

#[test]
fn a_reopened_document_continues_the_delta_chain() {
    let mut doc = doc_with_one_set();
    doc.commit().expect("commit");
    let compacted = doc.export_compacted().expect("compact");

    let mut reopened = DataDoc::new();
    reopened.import(&compacted).expect("import");
    reopened.mark_persisted();
    reopened
        .map_set("profile", "city", DataValue::text("london"))
        .expect("set");
    let delta = reopened.commit().expect("commit").expect("a change");

    // A peer holding only the compacted record must be brought up to date
    // by that delta alone.
    let mut peer = DataDoc::new();
    peer.import(&compacted).expect("import compacted");
    peer.import(&delta.bytes).expect("import delta");
    assert_eq!(
        peer.map_get("profile", "city").expect("get"),
        Some(DataValue::text("london"))
    );
}

#[test]
fn corrupt_bytes_are_refused_rather_than_applied() {
    let mut doc = DataDoc::new();
    let err = doc
        .import(b"this is not an encoded document")
        .expect_err("garbage must be refused");
    assert!(matches!(err, DataError::Corrupt(_)), "got {err:?}");
}

#[test]
fn a_truncated_delta_is_refused() {
    let mut source = doc_with_one_set();
    let delta = source.commit().expect("commit").expect("a change");
    let truncated = &delta.bytes[..delta.bytes.len() / 2];

    let mut replica = DataDoc::new();
    let err = replica
        .import(truncated)
        .expect_err("truncation is corrupt");
    assert!(matches!(err, DataError::Corrupt(_)), "got {err:?}");
}

#[test]
fn a_bit_flip_in_a_delta_is_refused() {
    let mut source = doc_with_one_set();
    let delta = source.commit().expect("commit").expect("a change");
    let mut flipped = delta.bytes.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xff;

    let mut replica = DataDoc::new();
    assert!(replica.import(&flipped).is_err(), "a flipped byte applied");
}

#[test]
fn every_value_kind_survives_a_round_trip() {
    let mut doc = DataDoc::new();
    let cases = [
        ("null", DataValue::Null),
        ("bool", DataValue::bool(true)),
        ("int", DataValue::int(-42)),
        ("float", DataValue::float(1.5)),
        ("text", DataValue::text("hello")),
        ("bytes", DataValue::bytes(vec![0u8, 1, 255])),
    ];
    for (key, value) in &cases {
        doc.map_set("values", key, value.clone()).expect("set");
    }
    let delta = doc.commit().expect("commit").expect("a change");

    let mut replica = DataDoc::new();
    replica.import(&delta.bytes).expect("import");
    for (key, value) in &cases {
        assert_eq!(
            replica.map_get("values", key).expect("get").as_ref(),
            Some(value),
            "{key} did not survive"
        );
    }
}

#[test]
fn deleting_a_map_key_removes_it() {
    let mut doc = doc_with_one_set();
    doc.commit().expect("commit");
    doc.map_delete("profile", "name").expect("delete");
    doc.commit().expect("commit");
    assert_eq!(doc.map_get("profile", "name").expect("get"), None);
}

#[test]
fn list_positions_outside_the_collection_are_refused() {
    let mut doc = DataDoc::new();
    doc.list_push("log", DataValue::int(1)).expect("push");

    let err = doc
        .list_insert("log", 5, DataValue::int(2))
        .expect_err("out of range");
    assert!(matches!(err, DataError::OutOfRange { .. }), "got {err:?}");

    let err = doc.list_delete("log", 0, 4).expect_err("out of range");
    assert!(matches!(err, DataError::OutOfRange { .. }), "got {err:?}");
}

#[test]
fn text_positions_outside_the_collection_are_refused() {
    let mut doc = DataDoc::new();
    doc.text_insert("body", 0, "hi").expect("insert");

    let err = doc.text_insert("body", 9, "x").expect_err("out of range");
    assert!(matches!(err, DataError::OutOfRange { .. }), "got {err:?}");

    let err = doc.text_delete("body", 1, 8).expect_err("out of range");
    assert!(matches!(err, DataError::OutOfRange { .. }), "got {err:?}");
}

#[test]
fn names_that_would_break_a_record_key_are_refused() {
    for bad in ["", "a/b", "../etc", "with space", &"x".repeat(129)] {
        assert!(
            crate::validate_name(bad).is_err(),
            "{bad:?} should be refused"
        );
    }
    for good in ["docs", "my-doc", "my_doc.v2", "A1"] {
        crate::validate_name(good).expect("should be accepted");
    }
}

#[test]
fn a_bad_collection_name_is_refused_at_the_operation() {
    let mut doc = DataDoc::new();
    let err = doc
        .map_set("space/doc", "k", DataValue::int(1))
        .expect_err("separator in a collection name");
    assert!(matches!(err, DataError::InvalidName { .. }), "got {err:?}");
}

#[test]
fn oversized_map_keys_are_refused() {
    let mut doc = DataDoc::new();
    let key = "k".repeat(crate::MAX_KEY_LEN + 1);
    assert!(doc.map_set("m", &key, DataValue::int(1)).is_err());
    assert!(doc.map_set("m", "", DataValue::int(1)).is_err());
}

#[test]
fn json_export_shows_the_current_state() {
    let mut doc = DataDoc::new();
    doc.map_set("profile", "name", DataValue::text("ada"))
        .expect("set");
    doc.text_insert("body", 0, "notes").expect("insert");
    doc.commit().expect("commit");

    let json = doc.export_json().expect("json");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(parsed["profile"]["name"], serde_json::json!("ada"));
    assert_eq!(parsed["body"], serde_json::json!("notes"));
}

#[test]
fn raw_export_restores_a_document() {
    let mut doc = doc_with_one_set();
    doc.commit().expect("commit");
    let raw = doc.export_raw().expect("raw");

    let mut restored = DataDoc::new();
    restored.import(&raw).expect("import raw");
    assert_eq!(
        restored.map_get("profile", "name").expect("get"),
        Some(DataValue::text("ada"))
    );
}

#[test]
fn a_single_field_delta_stays_inside_one_bluetooth_fragment() {
    // The measured figure the engine choice rests on: one map set encodes
    // to well under the 139 usable bytes of an unnegotiated BLE fragment.
    // If a change here pushes a single-field update past that, the layer
    // stops being usable on the weakest transport and the engine decision
    // has to be re-opened rather than absorbed.
    let mut doc = DataDoc::new();
    doc.map_set("m", "k", DataValue::int(1)).expect("set");
    let delta = doc.commit().expect("commit").expect("a change");
    assert!(
        delta.bytes.len() < 139,
        "single-field delta grew to {} bytes",
        delta.bytes.len()
    );
}

#[test]
fn compaction_keeps_a_busy_document_far_smaller_than_its_history() {
    let mut doc = DataDoc::new();
    for index in 0..2_000 {
        doc.map_set("counters", "hot", DataValue::int(index))
            .expect("set");
        if index % 50 == 0 {
            doc.commit().expect("commit");
        }
    }
    doc.commit().expect("commit");

    let compacted = doc.export_compacted().expect("compact").len();
    let full = doc.export_raw().expect("raw").len();
    assert!(
        compacted * 4 < full,
        "compaction bought little: {compacted} vs {full}"
    );
}

#[test]
fn size_checks_stay_quiet_for_a_small_document() {
    let mut doc = doc_with_one_set();
    doc.commit().expect("commit");
    let (verdict, measured) = doc.check_size().expect("check");
    assert_eq!(verdict, policy::SizeVerdict::Ok);
    assert!(
        measured.is_none(),
        "a small document should not pay for a measurement"
    );
}

#[test]
fn a_rewound_commit_is_handed_out_again() {
    // The failure this pins: a caller whose persistence failed has already
    // had the export marker advanced by commit(), so without a rewind the
    // change is never offered again. The document still shows it, so nothing
    // looks wrong until a restart, when it is simply not there.
    let mut doc = DataDoc::new();
    doc.map_set("m", "k", DataValue::text("first"))
        .expect("set");
    let first = doc.commit().expect("commit").expect("a change");

    doc.rewind_last_commit();

    doc.map_set("m", "k2", DataValue::text("second"))
        .expect("set");
    let second = doc.commit().expect("commit").expect("a change");

    // The second delta must carry BOTH changes: the rewound one and the new
    // one. A replica that only ever saw the second delta has to end up whole.
    let mut replica = DataDoc::new();
    replica.import(&second.bytes).expect("import");
    assert_eq!(
        replica.map_get("m", "k").expect("get"),
        Some(DataValue::text("first")),
        "the rewound change was not re-exported"
    );
    assert_eq!(
        replica.map_get("m", "k2").expect("get"),
        Some(DataValue::text("second"))
    );
    // And the original delta stays valid, so a rewind after a write that
    // actually succeeded costs a duplicate, not a conflict.
    replica.import(&first.bytes).expect("re-import the first");
    assert_eq!(
        replica.map_get("m", "k").expect("get"),
        Some(DataValue::text("first"))
    );
}

#[test]
fn rewinding_without_a_commit_is_harmless() {
    let mut doc = DataDoc::new();
    doc.rewind_last_commit();
    doc.map_set("m", "k", DataValue::int(1)).expect("set");
    let delta = doc.commit().expect("commit").expect("a change");

    let mut replica = DataDoc::new();
    replica.import(&delta.bytes).expect("import");
    assert_eq!(
        replica.map_get("m", "k").expect("get"),
        Some(DataValue::int(1))
    );
}

#[test]
fn a_reopened_document_does_not_forget_how_big_it_is() {
    // The bug this pins: a document loaded from a stored snapshot starts
    // with a zeroed size estimate, so the cheap check answers "no need to
    // measure" no matter how large it actually is. A document already near
    // the cap would then sail past it, and the first symptom would be a
    // record that will not fit.
    let mut doc = DataDoc::new();
    doc.map_set("m", "k", DataValue::int(1)).expect("set");
    doc.commit().expect("commit");
    let compacted = doc.export_compacted().expect("compact");

    let mut reopened = DataDoc::new();
    reopened.import(&compacted).expect("import");
    reopened.mark_persisted();

    // Stand in for a document loaded at three quarters of the cap.
    reopened.restore_bookkeeping(policy::DOC_SIZE_WARN_BYTES, 0, 0);
    let (_, measured) = reopened.check_size().expect("check");
    assert!(
        measured.is_some(),
        "a document restored near the cap must be measured, not assumed small"
    );
}

#[test]
fn restored_bookkeeping_counts_the_delta_log_toward_the_estimate() {
    let mut doc = DataDoc::new();
    doc.map_set("m", "k", DataValue::int(1)).expect("set");
    doc.commit().expect("commit");

    // A small stored snapshot with a large replayed log is still a large
    // document: the estimate has to add them, or the log is invisible until
    // the next compaction.
    doc.restore_bookkeeping(1024, policy::DOC_SIZE_WARN_BYTES, 7);
    let (_, measured) = doc.check_size().expect("check");
    assert!(measured.is_some(), "the replayed log was not counted");
    assert_eq!(
        doc.commits_since_compaction(),
        7,
        "the replayed commit count drives the compaction trigger"
    );
}

#[test]
fn stats_report_the_commit_count_since_compaction() {
    let mut doc = DataDoc::new();
    for index in 0..3 {
        doc.map_set("m", "k", DataValue::int(index)).expect("set");
        doc.commit().expect("commit");
    }
    let stats = doc.stats().expect("stats");
    assert_eq!(stats.commits_since_compaction, 3);
    assert!(stats.compacted_bytes > 0);

    doc.mark_compacted(stats.compacted_bytes);
    assert_eq!(doc.commits_since_compaction(), 0);
}
