//! The aggregates exist only inside the pipe.
//!
//! The rollup and the session summary are the part of the client worth
//! copying, and the free event API hands every protocol event to app code
//! as opaque JSON. The product boundary is therefore a rule, not a lock:
//! no aggregate wire type is ever an `Event`, reaches `on_event`, or is
//! constructed anywhere but the pipe. This pins the rule.

use offline_protocol_telemetry_wire::event::{TYPE_METRICS_ROLLUP, TYPE_SESSION_SUMMARY};

use crate::telemetry::record::tests::{event_exemplars, ALL_TELEMETRY_NAMES};

#[test]
fn aggregate_wire_types_never_reach_the_free_event_api() {
    for aggregate in [TYPE_METRICS_ROLLUP, TYPE_SESSION_SUMMARY] {
        assert!(
            !ALL_TELEMETRY_NAMES.contains(&aggregate),
            "{aggregate} must not be a telemetry record name"
        );
        for event in event_exemplars() {
            assert_ne!(
                event.telemetry_name(),
                aggregate,
                "{aggregate} must not be an Event"
            );
        }
    }

    // Nothing outside the pipe spells either type: the constructors live in
    // `telemetry/pipe/` and the names in the wire crate. A literal anywhere
    // else in this crate is a second producer.
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    walk(&src, &mut |path, contents| {
        if path.components().any(|c| c.as_os_str() == "pipe") {
            return;
        }
        for aggregate in [TYPE_METRICS_ROLLUP, TYPE_SESSION_SUMMARY] {
            if contents.contains(aggregate) {
                offenders.push(format!("{}: {aggregate}", path.display()));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "aggregate wire types are named outside the pipe: {offenders:?}"
    );
}

fn walk(dir: &std::path::Path, visit: &mut dyn FnMut(&std::path::Path, &str)) {
    for entry in std::fs::read_dir(dir).expect("readable source tree") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(&path, visit);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let contents = std::fs::read_to_string(&path).expect("readable source");
            visit(&path, &contents);
        }
    }
}
