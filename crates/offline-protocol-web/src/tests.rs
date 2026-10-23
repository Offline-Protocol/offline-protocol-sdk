//! Two engines on one host, joined by the links a page would carry.

use std::sync::Arc;
use std::time::Duration;

use crate::log_store::{Category, LogStore, MemoryFile, STORE_KEY_BYTES};
use crate::node::{LinkId, Node, MAX_LINKS};

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
        .send_message(&b_addr, "hello from a tab", false)
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
    a.node
        .send_group_message(&group, "serve", false)
        .expect("send");
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
    a.node
        .send_message(&b_addr, "still here", false)
        .expect("send");
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

#[test]
fn a_frame_for_a_link_that_closed_is_dropped_not_misdelivered() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| saw(
        a,
        "secure_session_established",
        "peer_id",
        &b_addr
    )));
    // Queued for b's link, which then closes before the page drains it.
    a.node
        .send_message(&b_addr, "too late", false)
        .expect("send");
    a.node.close_link(a.link);
    assert_eq!(a.node.next_outbound(), None);
}

#[test]
fn links_past_the_ceiling_are_refused() {
    let mut a = peer("alice");
    // `peer` already opened one.
    for _ in 1..MAX_LINKS {
        a.node.open_link().expect("under the ceiling");
    }
    assert!(
        a.node.open_link().is_err(),
        "a link past the ceiling opened"
    );
    a.node.close_link(a.link);
    a.node.open_link().expect("a closed link frees its place");
}

#[test]
fn a_link_that_never_proves_anyone_is_closed_and_reported() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| saw(
        a,
        "neighbor_discovered",
        "peer_id",
        &b_addr
    )));
    // A second carrier opens, takes our preamble, and never answers.
    a.node.set_preamble_deadline(Duration::from_millis(50));
    let silent = a.node.open_link().expect("link");
    let (link, _preamble) = a.node.next_outbound().expect("our preamble");
    assert_eq!(link, silent);
    std::thread::sleep(Duration::from_millis(60));
    a.node.process().expect("process");
    assert_eq!(a.node.next_closed(), Some(silent));
    assert_eq!(a.node.next_closed(), None, "the proved link was closed too");
    assert_eq!(
        a.node.link_received(silent, &[0, 0, 0, 1, 0]).as_deref(),
        Some("unknown link")
    );
    // The proved link is untouched.
    a.node
        .send_message(&b_addr, "still here", false)
        .expect("send");
    assert!(run_until(&mut a, &mut b, |_, b| {
        b.events
            .iter()
            .any(|e| e["type"] == "message_received" && e["content"] == "still here")
    }));
}

/// Reads a repository file with its line endings normalised. A Windows
/// checkout writes CRLF into every file `.gitattributes` does not pin, which
/// includes `Cargo.lock` and the workflows, and a guard that matches `\n`
/// against those fails there for a reason unrelated to what it guards.
fn read_repo_file(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.replace("\r\n", "\n"))
}

/// The wasm-bindgen crate and its command-line tool share an unversioned
/// schema, so every place that names the version must name the same one: the
/// lockfile, this crate's `=` pin, the build script, and CI's install. A
/// mismatch fails only when the module is post-processed, which a host build
/// never does.
#[test]
fn the_wasm_bindgen_pins_agree() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Some(lock) = read_repo_file(&root.join("Cargo.lock")) else {
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
        let text = read_repo_file(&root.join(file)).expect(file);
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
    let Some(ci) = read_repo_file(&root.join(".github/workflows/ci.yml")) else {
        return;
    };
    for needle in [
        "  web-binding:\n",
        "targets: wasm32-unknown-unknown",
        "bash bindings/web/scripts/build.sh",
        "bash bindings/web/scripts/test-browser.sh",
        "npx playwright install --with-deps chromium firefox webkit",
        "bash bindings/web/scripts/test-conformance.sh",
    ] {
        assert!(
            ci.contains(needle),
            "ci.yml no longer has `{}`",
            needle.trim()
        );
    }
}

/// The page remembers a signalling session until the newest offer it has
/// seen for it stops being fresh, by the bound the worker reports, rather
/// than for a fixed time from first sight. A fixed hold forgets a session
/// whose republished offer, or whose joiner's fast clock, keeps it fresh
/// longer, and a relay's late repeat then opens a second connection (W11).
/// The bound itself is pinned by
/// `an_opened_signal_says_how_long_a_relay_can_repeat_it`.
#[test]
fn the_host_remembers_a_session_as_long_as_its_offer_is_fresh() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Some(page) = read_repo_file(&root.join("bindings/web/src/nostr-signal.js")) else {
        return;
    };
    let host = page
        .split_once("async host(")
        .and_then(|(_, rest)| rest.split_once("async join("))
        .map(|(host, _)| host)
        .expect("nostr-signal.js defines host() before join()");
    for repeat_or_first in [
        "hold(signal.session, known, signal.freshUntilMs)",
        "hold(signal.session, entry, signal.freshUntilMs)",
    ] {
        assert!(
            host.contains(repeat_or_first),
            "host() does not hold a session by the offer's freshness: `{repeat_or_first}`"
        );
    }
    assert!(
        !page.contains("SESSION_HOLD_MS"),
        "nostr-signal.js holds a session for a fixed time again"
    );
}

/// The files of one node's two logs, kept across a restart.
struct Disk {
    mls: (MemoryFile, MemoryFile),
    state: (MemoryFile, MemoryFile),
}

impl Disk {
    fn new() -> Self {
        Self {
            mls: Default::default(),
            state: Default::default(),
        }
    }

    fn boot(&self, profile: &str) -> Peer {
        self.boot_measured(profile).0
    }

    /// [`Self::boot`], with the two logs for a test that reads their costs.
    fn boot_measured(&self, profile: &str) -> (Peer, [Arc<LogStore<MemoryFile>>; 2]) {
        let key = [42u8; STORE_KEY_BYTES];
        let mls = LogStore::open(&key, Category::Mls, self.mls.0.clone(), self.mls.1.clone())
            .expect("mls log");
        let state = LogStore::open(
            &key,
            Category::ProtocolState,
            self.state.0.clone(),
            self.state.1.clone(),
        )
        .expect("state log");
        let (mls, state) = (Arc::new(mls), Arc::new(state));
        let mut node = Node::new("web-test", profile).expect("node");
        let address = node
            .initialize_with(mls.clone(), state.clone())
            .expect("initialize");
        node.start().expect("start");
        let link = node.open_link().expect("link");
        let peer = Peer {
            node,
            link,
            events: Vec::new(),
            address,
        };
        (peer, [mls, state])
    }
}

#[test]
fn a_node_killed_and_restarted_over_its_logs_is_the_same_member() {
    let mut a = peer("host");
    let disk = Disk::new();
    let mut b = disk.boot("player");
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| {
        saw(a, "secure_session_established", "peer_id", &b_addr)
    }));
    let info: serde_json::Value =
        serde_json::from_str(&a.node.create_group("lobby").expect("group")).expect("json");
    let group = info["group_id"].as_str().expect("group id").to_string();
    let mut invited = false;
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
    a.node
        .send_group_message(&group, "before", false)
        .expect("send");
    assert!(run_until(&mut a, &mut b, |_, b| {
        b.events
            .iter()
            .any(|e| e["type"] == "group_message_received" && e["content"] == "before")
    }));

    // A crash: b is dropped without stop, and a's carrier to it closes.
    drop(b);
    a.node.close_link(a.link);
    a.link = a.node.open_link().expect("link");
    a.events.clear();

    let mut b = disk.boot("player");
    assert_eq!(
        b.address, b_addr,
        "a restart must load the identity, not mint one"
    );
    assert!(run_until(&mut a, &mut b, |a, _| {
        saw(a, "neighbor_discovered", "peer_id", &b_addr)
    }));
    a.node
        .send_group_message(&group, "after", false)
        .expect("send");
    assert!(
        run_until(&mut a, &mut b, |_, b| {
            b.events
                .iter()
                .any(|e| e["type"] == "group_message_received" && e["content"] == "after")
        }),
        "the restarted member could not read the group: {:?}",
        b.events
    );
    assert!(
        !b.events
            .iter()
            .any(|e| e["type"] == "message_decryption_failed"),
        "{:?}",
        b.events
    );
}

/// Store flushes per message for one member at 10 Hz-style sends: what an
/// ephemeral send saves against the acknowledged ladder (W10).
#[test]
fn an_ephemeral_group_send_costs_fewer_flushes_than_a_reliable_one() {
    let disk = Disk::new();
    let (mut a, logs) = disk.boot_measured("host");
    let mut b = peer("player");
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| {
        saw(a, "secure_session_established", "peer_id", &b_addr)
    }));
    let info: serde_json::Value =
        serde_json::from_str(&a.node.create_group("pong").expect("group")).expect("json");
    let group = info["group_id"].as_str().expect("group id").to_string();
    let mut invited = false;
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
    assert!(run_until(&mut a, &mut b, |_, _| true));

    let flushes = |_: &Peer| logs.iter().map(|log| log.stats().flushes).sum::<u64>();
    const SENDS: usize = 20;
    let measure = |a: &mut Peer, b: &mut Peer, ephemeral: bool, tag: &str| {
        let before = flushes(a);
        for n in 0..SENDS {
            a.node
                .send_group_message(&group, &format!("{tag} {n}"), ephemeral)
                .expect("send");
            let wanted = format!("{tag} {n}");
            assert!(run_until(a, b, |_, b| b.events.iter().any(|e| e["type"]
                == "group_message_received"
                && e["content"] == wanted.as_str())));
        }
        // Let acknowledgements land and the outbox settle.
        for _ in 0..8 {
            run_until(a, b, |_, _| true);
        }
        (flushes(a) - before) as f64 / SENDS as f64
    };
    let reliable = measure(&mut a, &mut b, false, "reliable");
    let ephemeral = measure(&mut a, &mut b, true, "ephemeral");
    eprintln!("flushes per group message: reliable {reliable:.2}, ephemeral {ephemeral:.2}");
    assert!(
        ephemeral < reliable,
        "reliable {reliable:.2} vs ephemeral {ephemeral:.2}"
    );
}

#[test]
fn an_ephemeral_direct_message_with_no_link_fails_at_once() {
    let (mut a, mut b) = (peer("alice"), peer("bob"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut a, &mut b, |a, _| {
        saw(a, "secure_session_established", "peer_id", &b_addr)
    }));
    a.node
        .send_message(&b_addr, "paddle 4", true)
        .expect("linked and confirmed");
    assert!(run_until(&mut a, &mut b, |_, b| b
        .events
        .iter()
        .any(
            |e| e["type"] == "message_received" && e["content"] == "paddle 4"
        )));
    a.node.close_link(a.link);
    let error = a
        .node
        .send_message(&b_addr, "paddle 5", true)
        .expect_err("no link");
    assert!(error.0.contains("live direct link"), "{}", error.0);
}
