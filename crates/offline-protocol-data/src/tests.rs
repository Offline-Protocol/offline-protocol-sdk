//! Unit tests for the replicated-document layer.

use crate::doc::{CatchUp, DataDoc, ImportOutcome, RemoteImport};
use crate::error::DataError;
use crate::policy;
use crate::value::DataValue;
use crate::MAX_VALUE_BYTES;

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

#[test]
fn a_delta_whose_predecessor_is_missing_is_reported_as_parked() {
    let mut source = DataDoc::new();
    source.map_set("m", "k1", DataValue::int(1)).expect("set");
    let first = source.commit().expect("commit").expect("a delta");
    source.map_set("m", "k2", DataValue::int(2)).expect("set");
    let second = source.commit().expect("commit").expect("a delta");

    // Only the second delta arrives. The engine takes it and answers Ok, so
    // the outcome is the only thing that distinguishes "applied" from "held
    // out of sight" — and an owner that cannot tell them apart will compact
    // this document and delete the record it is waiting for.
    let mut replica = DataDoc::new();
    let outcome = replica.import(&second.bytes).expect("import");
    assert_eq!(
        outcome,
        ImportOutcome::Parked,
        "an orphan delta was reported as applied"
    );
    assert_eq!(
        replica.map_get("m", "k2").expect("get"),
        None,
        "a parked change must not read as present"
    );

    // And it must stay out of a compacted export, which is what makes
    // compacting-then-deleting destructive here.
    let compacted = replica.export_compacted().expect("export");
    let mut round_trip = DataDoc::new();
    round_trip.import(&compacted).expect("import");
    assert_eq!(round_trip.map_get("m", "k2").expect("get"), None);

    // Once the predecessor lands, both apply.
    let outcome = replica.import(&first.bytes).expect("import");
    assert_eq!(outcome, ImportOutcome::Applied);
    assert_eq!(
        replica.map_get("m", "k2").expect("get"),
        Some(DataValue::int(2)),
        "the parked change did not apply once its predecessor arrived"
    );
}

#[test]
fn a_complete_delta_is_reported_as_applied() {
    let mut source = DataDoc::new();
    source.map_set("m", "k", DataValue::int(7)).expect("set");
    let delta = source.commit().expect("commit").expect("a delta");

    let mut replica = DataDoc::new();
    assert_eq!(
        replica.import(&delta.bytes).expect("import"),
        ImportOutcome::Applied,
        "a delta with no missing predecessor was reported as parked"
    );
}

#[test]
fn a_value_larger_than_any_document_is_refused() {
    let mut doc = DataDoc::new();
    let huge = "x".repeat(MAX_VALUE_BYTES + 1);

    assert!(matches!(
        doc.map_set("m", "k", DataValue::text(huge.clone())),
        Err(DataError::ValueTooLarge { .. })
    ));
    assert!(matches!(
        doc.list_push("l", DataValue::bytes(vec![0u8; MAX_VALUE_BYTES + 1])),
        Err(DataError::ValueTooLarge { .. })
    ));
    assert!(matches!(
        doc.list_insert("l", 0, DataValue::text(huge.clone())),
        Err(DataError::ValueTooLarge { .. })
    ));
    assert!(matches!(
        doc.text_insert("t", 0, &huge),
        Err(DataError::ValueTooLarge { .. })
    ));

    // The refusal is at the operation, so nothing reached the document.
    assert_eq!(doc.map_get("m", "k").expect("get"), None);
    assert!(doc.commit().expect("commit").is_none());
}

/// A document holding `commits` separate commits, plus the deltas it made,
/// in commit order.
fn source_with_commits(commits: usize) -> (DataDoc, Vec<Vec<u8>>) {
    let mut doc = DataDoc::new();
    let mut deltas = Vec::new();
    for n in 0..commits {
        doc.map_set("m", &format!("k{n}"), DataValue::int(n as i64))
            .expect("set");
        deltas.push(doc.commit().expect("commit").expect("a delta").bytes);
    }
    (doc, deltas)
}

/// A replica that has applied `deltas` and then compacted, so its history is
/// trimmed at its current frontier: the shape a document has after every
/// ordinary compaction pass, and the shape the engine mishandles.
fn compacted_replica(deltas: &[Vec<u8>]) -> DataDoc {
    let mut staging = DataDoc::new();
    for delta in deltas {
        staging.import(delta).expect("import");
    }
    let compacted = staging.export_compacted().expect("compact");

    let mut replica = DataDoc::new();
    replica.import(&compacted).expect("import compacted");
    replica
}

#[test]
fn a_redelivered_delta_never_reaches_a_compacted_document() {
    let (_source, deltas) = source_with_commits(2);
    let mut replica = compacted_replica(&deltas);

    // The ladder promises at-least-once, so this redelivery is ordinary
    // traffic and not an attack. It is also the exact shape that refers to
    // history the compaction trimmed away, which the engine answers with a
    // panic rather than an error (loro #1068) and which `panic = "abort"`
    // turns into a dead process on mobile.
    let meta = replica.inspect(&deltas[1]).expect("inspect");
    assert!(
        meta.already_applied(),
        "a delta already folded into the snapshot was not recognized"
    );
    assert_eq!(
        replica.import_remote(&deltas[1]).expect("import remote"),
        RemoteImport::AlreadyHave,
        "a redelivered delta was handed to the engine"
    );

    // The document is untouched and still readable, which is the point.
    assert_eq!(
        replica.map_get("m", "k1").expect("get"),
        Some(DataValue::int(1))
    );
    assert!(!replica.is_poisoned());
}

#[test]
fn a_blob_spanning_compacted_history_is_refused_rather_than_imported() {
    let (mut source, deltas) = source_with_commits(2);
    let mut replica = compacted_replica(&deltas);

    // A third change the replica has not seen, encoded from the beginning of
    // history: partly new, but reaching back below the replica's trim point.
    // No run of updates can express that gap, so the only safe answer is to
    // decline and ask for a snapshot.
    source.map_set("m", "k2", DataValue::int(2)).expect("set");
    source.commit().expect("commit").expect("a delta");
    let from_scratch = match source
        .export_since(&DataDoc::new().version())
        .expect("export since nothing")
    {
        CatchUp::Updates(bytes) => bytes,
        other => panic!("expected updates, got {other:?}"),
    };

    let meta = replica.inspect(&from_scratch).expect("inspect");
    assert!(
        !meta.already_applied(),
        "the blob carries a change the replica lacks"
    );
    assert!(
        meta.spans_trimmed_history(),
        "a blob reaching below the trim point was not recognized"
    );
    assert_eq!(
        replica.import_remote(&from_scratch).expect("import remote"),
        RemoteImport::RefusedTrimmedHistory,
        "a blob spanning trimmed history was handed to the engine"
    );
    assert!(!replica.is_poisoned());

    // A snapshot carries its own base, so it is the answer to that refusal.
    let snapshot = source.export_raw().expect("snapshot");
    assert!(replica.inspect(&snapshot).expect("inspect").is_snapshot());
    assert_eq!(
        replica.import_remote(&snapshot).expect("import snapshot"),
        RemoteImport::Applied
    );
    assert_eq!(
        replica.map_get("m", "k2").expect("get"),
        Some(DataValue::int(2)),
        "the snapshot did not close the gap"
    );
}

#[test]
fn a_fresh_remote_delta_applies_and_a_parked_one_is_reported() {
    let (_source, deltas) = source_with_commits(2);

    let mut replica = DataDoc::new();
    assert_eq!(
        replica.import_remote(&deltas[1]).expect("import"),
        RemoteImport::Parked,
        "an orphan remote delta was reported as applied"
    );
    assert_eq!(
        replica.import_remote(&deltas[0]).expect("import"),
        RemoteImport::Applied
    );
    assert_eq!(
        replica.map_get("m", "k1").expect("get"),
        Some(DataValue::int(1)),
        "the parked change did not apply once its predecessor arrived"
    );
}

#[test]
fn corrupt_remote_bytes_are_refused_without_poisoning_the_document() {
    let (_source, deltas) = source_with_commits(1);
    let mut replica = DataDoc::new();

    let mut corrupt = deltas[0].clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0xff;
    assert!(matches!(
        replica.import_remote(&corrupt),
        Err(DataError::Corrupt(_))
    ));
    assert!(matches!(
        replica.inspect(b"not a blob at all"),
        Err(DataError::Corrupt(_))
    ));

    // Refusing junk must leave the handle usable: a peer that can poison a
    // document by sending one bad frame has a denial of service.
    assert!(!replica.is_poisoned());
    assert_eq!(
        replica.import_remote(&deltas[0]).expect("import"),
        RemoteImport::Applied
    );
}

#[test]
fn catch_up_serves_exactly_what_a_peer_is_missing() {
    let (mut source, deltas) = source_with_commits(1);

    let mut peer = DataDoc::new();
    peer.import(&deltas[0]).expect("import");
    let peer_version = peer.version();

    // Nothing new yet.
    assert_eq!(
        source.export_since(&peer_version).expect("export since"),
        CatchUp::UpToDate
    );

    source
        .map_set("m", "later", DataValue::int(9))
        .expect("set");
    source.commit().expect("commit").expect("a delta");

    let updates = match source.export_since(&peer_version).expect("export since") {
        CatchUp::Updates(bytes) => bytes,
        other => panic!("expected updates, got {other:?}"),
    };
    assert_eq!(
        peer.import_remote(&updates).expect("import"),
        RemoteImport::Applied,
        "catch-up updates did not apply cleanly"
    );
    assert_eq!(
        peer.map_get("m", "later").expect("get"),
        Some(DataValue::int(9))
    );

    // And a peer that already has everything is told so rather than being
    // sent bytes: on a mesh the difference is a frame per idle reconnect.
    assert_eq!(
        source.export_since(&peer.version()).expect("export since"),
        CatchUp::UpToDate
    );
}

#[test]
fn catch_up_from_before_a_compaction_asks_for_a_snapshot() {
    let (_source, deltas) = source_with_commits(2);
    let mut replica = compacted_replica(&deltas);
    replica.map_set("m", "own", DataValue::int(5)).expect("set");
    replica.commit().expect("commit").expect("a delta");

    // A peer that has nothing cannot be served from a document whose early
    // history was compacted away. Answering with updates anyway is the
    // sending half of the same defect `import_remote` declines.
    assert_eq!(
        replica
            .export_since(&DataDoc::new().version())
            .expect("export since nothing"),
        CatchUp::NeedsSnapshot
    );
}

#[test]
fn a_malformed_version_token_is_refused_rather_than_answered_with_everything() {
    let (source, _deltas) = source_with_commits(1);
    let garbled = crate::doc::VersionToken::from_bytes(vec![0xff; 16]);

    // Treating an undecodable token as "knows nothing" would make one
    // corrupt byte an amplification lever on the sync path.
    assert!(matches!(
        source.export_since(&garbled),
        Err(DataError::Corrupt(_))
    ));
}

#[test]
fn a_branch_forked_below_the_trim_point_never_reaches_the_engine() {
    // The shape loro #1068 names, and the reason this gate is structural
    // rather than defensive. Verified on 1.13.9 by removing the refusal:
    // the engine panics in `pending_changes.rs` and the retry panics again
    // on the mutex that first panic poisoned. Under the `minisize` profile
    // the SDK ships with `panic = "abort"`, so there is no catch to reach.
    let (_source, deltas) = source_with_commits(2);

    // A replica that saw only the first change, then edited concurrently.
    // Its change depends on history the compacted replica has trimmed and
    // is not an ancestor of anything that replica holds.
    let mut fork = DataDoc::new();
    fork.import(&deltas[0]).expect("seed the fork");
    fork.map_set("m", "fork", DataValue::int(42)).expect("set");
    let fork_delta = fork.commit().expect("commit").expect("a delta").bytes;

    let mut replica = compacted_replica(&deltas);
    assert!(
        replica
            .inspect(&fork_delta)
            .expect("inspect")
            .spans_trimmed_history(),
        "a branch forked below the trim point was not recognized"
    );
    assert_eq!(
        replica.import_remote(&fork_delta).expect("import remote"),
        RemoteImport::RefusedTrimmedHistory,
        "a branch forked below the trim point was handed to the engine"
    );

    // The document survives, which is the whole point: refusing costs one
    // extra round trip, and importing costs the process.
    assert!(!replica.is_poisoned());
    assert_eq!(
        replica.map_get("m", "k1").expect("get"),
        Some(DataValue::int(1))
    );
}

#[test]
fn an_ordinary_delta_after_a_compaction_still_applies() {
    // The other half of the trim gate: it must refuse the forked branch
    // above without refusing steady-state traffic. A gate that answered
    // "send a snapshot" to every delta a synced peer produces after the
    // first compaction would be correct and useless.
    let mut alice = DataDoc::new();
    alice.map_set("m", "a", DataValue::int(0)).expect("set");
    let from_alice = alice.commit().expect("commit").expect("a delta").bytes;

    let mut bob = DataDoc::new();
    bob.import(&from_alice).expect("import");
    bob.map_set("m", "b", DataValue::int(1)).expect("set");
    let from_bob = bob.commit().expect("commit").expect("a delta").bytes;
    alice.import(&from_bob).expect("import");

    // Alice compacts, as every flush eventually does.
    let compacted = alice.export_compacted().expect("compact");
    let mut alice = DataDoc::new();
    alice.import(&compacted).expect("reopen compacted");

    bob.map_set("m", "b2", DataValue::int(2)).expect("set");
    let next = bob.commit().expect("commit").expect("a delta").bytes;

    assert_eq!(
        alice.import_remote(&next).expect("import remote"),
        RemoteImport::Applied,
        "an ordinary delta was refused because the receiver had compacted"
    );
    assert_eq!(
        alice.map_get("m", "b2").expect("get"),
        Some(DataValue::int(2))
    );
}
