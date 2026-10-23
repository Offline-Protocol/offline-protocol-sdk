//! Two engines on one host, joined by the links a page would carry.

use std::time::Duration;

use crate::node::{LinkId, Node};

struct Peer {
    node: Node,
    link: LinkId,
    events: Vec<serde_json::Value>,
    address: String,
}

fn peer(profile: &str) -> Peer {
    let mut node = Node::new("web-test", profile).expect("node");
    let address = node.initialize().expect("initialize");
    node.start().expect("start");
    let link = node.open_link().expect("link");
    Peer {
        node,
        link,
        events: Vec::new(),
        address,
    }
}

/// Moves every queued frame across, ticks both engines, and collects events,
/// until `done` holds or the deadline passes. Real time passes between ticks
/// because the engine's acknowledgement and retry timers are wall-clock.
fn run_until(a: &mut Peer, b: &mut Peer, mut done: impl FnMut(&Peer, &Peer) -> bool) -> bool {
    for _ in 0..400 {
        while let Some((link, data)) = a.node.next_outbound() {
            assert_eq!(link, a.link);
            assert_eq!(b.node.link_received(b.link, &data), None);
        }
        while let Some((link, data)) = b.node.next_outbound() {
            assert_eq!(link, b.link);
            assert_eq!(a.node.link_received(a.link, &data), None);
        }
        a.node.process().expect("process");
        b.node.process().expect("process");
        for p in [&mut *a, &mut *b] {
            while let Some(json) = p.node.poll_event() {
                p.events
                    .push(serde_json::from_str(&json).expect("event json"));
            }
        }
        if done(a, b) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn saw(p: &Peer, kind: &str, field: &str, value: &str) -> bool {
    p.events
        .iter()
        .any(|e| e["type"] == kind && e[field] == value)
}

#[test]
fn two_engines_announce_each_other_by_their_proved_addresses() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let (a_addr, b_addr) = (a.address.clone(), b.address.clone());
    assert!(run_until(&mut a, &mut b, |a, b| {
        saw(a, "neighbor_discovered", "peer_id", &b_addr)
            && saw(b, "neighbor_discovered", "peer_id", &a_addr)
    }));
}

#[test]
fn a_direct_message_arrives_end_to_end_encrypted() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| {
        saw(a, "secure_session_established", "peer_id", &b_addr)
    }));
    a.node
        .send_message(&b_addr, "hello from a tab")
        .expect("send");
    assert!(run_until(&mut a, &mut b, |_, b| {
        b.events.iter().any(|e| {
            e["type"] == "message_received"
                && e["content"] == "hello from a tab"
                && e["encrypted"] == true
        })
    }));
}

#[test]
fn a_group_message_reaches_an_invited_member() {
    let (mut a, mut b) = (peer("host"), peer("player"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| {
        saw(a, "secure_session_established", "peer_id", &b_addr)
    }));
    let info: serde_json::Value =
        serde_json::from_str(&a.node.create_group("lobby").expect("group")).expect("json");
    let group = info["group_id"].as_str().expect("group id").to_string();
    // The invite needs the key package each side sends after the session
    // forms, which may still be in flight.
    let mut invited = false;
    assert!(run_until(&mut a, &mut b, |_, _| true));
    for _ in 0..40 {
        if a.node.invite_to_group(&group, &b_addr).is_ok() {
            invited = true;
            break;
        }
        run_until(&mut a, &mut b, |_, _| true);
    }
    assert!(invited, "the invite never found the member's key package");
    assert!(run_until(&mut a, &mut b, |_, b| saw(
        b,
        "group_member_added",
        "group_id",
        &group
    )));
    a.node.send_group_message(&group, "serve").expect("send");
    assert!(run_until(&mut a, &mut b, |_, b| {
        b.events
            .iter()
            .any(|e| e["type"] == "group_message_received" && e["content"] == "serve")
    }));
}

#[test]
fn a_second_link_proving_an_announced_address_is_refused() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| saw(
        a,
        "neighbor_discovered",
        "peer_id",
        &b_addr
    )));
    // A second carrier from b toward a, carrying b's preamble again.
    let second_on_b = b.node.open_link().expect("link");
    let second_on_a = a.node.open_link().expect("link");
    let mut refusal = None;
    while let Some((link, data)) = b.node.next_outbound() {
        if link == second_on_b {
            refusal = a.node.link_received(second_on_a, &data);
        }
    }
    let refusal = refusal.expect("the duplicate link was accepted");
    assert!(refusal.contains("already announced"), "{refusal}");
    // The first link still carries traffic.
    a.node.send_message(&b_addr, "still here").expect("send");
    assert!(run_until(&mut a, &mut b, |_, b| {
        b.events
            .iter()
            .any(|e| e["type"] == "message_received" && e["content"] == "still here")
    }));
}

#[test]
fn a_link_whose_first_frame_is_not_a_preamble_closes_and_announces_nothing() {
    let mut a = peer("alice");
    let link = a.node.open_link().expect("link");
    let junk = offline_protocol_transport::frame(&[7u8; 120]).expect("frame");
    let reason = a.node.link_received(link, &junk).expect("not refused");
    assert!(reason.contains("identity assertion"), "{reason}");
    while let Some(json) = a.node.poll_event() {
        assert!(!json.contains("neighbor_discovered"), "{json}");
    }
    assert_eq!(
        a.node.link_received(link, &junk).as_deref(),
        Some("unknown link")
    );
}

#[test]
fn closing_an_announced_link_reports_the_peer_lost() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| saw(
        a,
        "neighbor_discovered",
        "peer_id",
        &b_addr
    )));
    a.node.close_link(a.link);
    let mut lost = false;
    while let Some(json) = a.node.poll_event() {
        lost |= json.contains("neighbor_lost") && json.contains(&b_addr);
    }
    assert!(lost);
}

/// The wasm-bindgen crate and its command-line tool share an unversioned
/// schema, so every place that names the version must name the same one: the
/// lockfile, this crate's `=` pin, the build script, and CI's install. A
/// mismatch fails only when the module is post-processed, which a host build
/// never does.
#[test]
fn the_wasm_bindgen_pins_agree() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(lock) = std::fs::read_to_string(root.join("Cargo.lock")) else {
        // A published .crate archive has no workspace around it.
        return;
    };
    let locked = lock
        .split("\n[[package]]\n")
        .find(|entry| entry.starts_with("name = \"wasm-bindgen\"\n"))
        .and_then(|entry| entry.lines().nth(1))
        .and_then(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("wasm-bindgen in Cargo.lock");

    for (file, needle) in [
        (
            "crates/offline-protocol-web/Cargo.toml",
            format!("wasm-bindgen = \"={locked}\""),
        ),
        (
            "bindings/web/scripts/build.sh",
            format!("WASM_BINDGEN_VERSION=\"{locked}\""),
        ),
        (
            ".github/workflows/ci.yml",
            format!("cargo install wasm-bindgen-cli --version {locked} --locked"),
        ),
    ] {
        let text = std::fs::read_to_string(root.join(file)).expect(file);
        assert!(
            text.contains(&needle),
            "{file} must name wasm-bindgen {locked}, the Cargo.lock version: expected `{needle}`"
        );
    }
}

/// The browser job is what proves the module runs; a rename that dropped it
/// from the workflow would leave every other job green.
#[test]
fn the_browser_tests_run_in_ci() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(ci) = std::fs::read_to_string(root.join(".github/workflows/ci.yml")) else {
        return;
    };
    for needle in [
        "  web-binding:\n",
        "targets: wasm32-unknown-unknown",
        "bash bindings/web/scripts/build.sh",
        "bash bindings/web/scripts/test-browser.sh",
    ] {
        assert!(
            ci.contains(needle),
            "ci.yml no longer has `{}`",
            needle.trim()
        );
    }
}
