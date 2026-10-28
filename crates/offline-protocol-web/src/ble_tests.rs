//! Two engines linked over their Bluetooth LE slots, on one host.
//!
//! Each side plays its half of the GATT contract by hand: the central reads
//! the peripheral's Device id and Identity and verifies them, the peripheral
//! verifies the central's hello, and every fragment either side queues is
//! handed to the other as a write or a notification would carry it. The
//! fragment codec runs on both sides, as it does between a browser and a
//! phone.

use std::time::Duration;

use serde_json::Value;

use crate::node::{LinkId, Node, NodeOptions, MAX_LINKS};

struct Side {
    node: Node,
    address: String,
    events: Vec<Value>,
}

fn side(profile: &str) -> Side {
    let options = NodeOptions {
        ble: true,
        ..Default::default()
    };
    let mut node = Node::with_options("web-ble-test", profile, options).expect("node");
    let address = node.initialize().expect("initialize");
    node.start().expect("start");
    Side {
        node,
        address,
        events: Vec::new(),
    }
}

/// What a page does once a link is subscribed and the hello is written: the
/// central verifies what the peripheral served, the peripheral verifies the
/// hello, and each tells its engine the link is up.
fn link(central: &mut Side, peripheral: &mut Side, payload: u32) {
    let identity = peripheral.node.ble_hello().expect("identity");
    let proved = central
        .node
        .ble_verify(peripheral.address.as_bytes(), &identity)
        .expect("the peripheral verifies");
    assert_eq!(proved, peripheral.address);
    central.node.ble_link_up(&proved, payload).expect("link up");

    let hello = central.node.ble_hello().expect("hello");
    let named = peripheral
        .node
        .ble_verify(central.address.as_bytes(), &hello)
        .expect("the hello verifies");
    peripheral
        .node
        .ble_link_up(&named, payload)
        .expect("link up");
}

fn pump(a: &mut Side, b: &mut Side) {
    while let Some((peer, fragment)) = a.node.ble_next_fragment() {
        assert_eq!(peer, b.address);
        b.node
            .ble_fragment_received(&a.address, &fragment)
            .expect("fragment");
    }
    while let Some((peer, fragment)) = b.node.ble_next_fragment() {
        assert_eq!(peer, a.address);
        a.node
            .ble_fragment_received(&b.address, &fragment)
            .expect("fragment");
    }
    for s in [&mut *a, &mut *b] {
        s.node.process().expect("process");
        while let Some(json) = s.node.poll_event() {
            s.events
                .push(serde_json::from_str(&json).expect("event json"));
        }
    }
}

fn run_until(a: &mut Side, b: &mut Side, mut done: impl FnMut(&Side, &Side) -> bool) -> bool {
    for _ in 0..400 {
        pump(a, b);
        if done(a, b) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn a_central_and_a_peripheral_form_a_session_and_exchange_a_message() {
    let (mut a, mut b) = (side("browser"), side("phone"));
    link(&mut a, &mut b, 244);
    let (a_addr, b_addr) = (a.address.clone(), b.address.clone());
    assert!(run_until(&mut a, &mut b, |a, b| {
        a.events
            .iter()
            .any(|e| e["type"] == "secure_session_established" && e["peer_id"] == b_addr.as_str())
            && b.events.iter().any(|e| {
                e["type"] == "secure_session_established" && e["peer_id"] == a_addr.as_str()
            })
    }));
    assert!(a
        .events
        .iter()
        .any(|e| e["type"] == "neighbor_discovered" && e["transport"] == "BLE"));
    // Longer than one 244-byte fragment, so it crosses in several.
    let long = "over Bluetooth ".repeat(40);
    a.node.send_message(&b_addr, &long, false).expect("send");
    assert!(run_until(&mut a, &mut b, |_, b| {
        b.events.iter().any(|e| {
            e["type"] == "message_received"
                && e["content"] == long.as_str()
                && e["encrypted"] == true
        })
    }));
}

#[test]
fn a_peripheral_whose_device_id_is_not_what_it_proves_is_refused() {
    let (mut a, b) = (side("browser"), side("phone"));
    let c = side("other");
    let identity = b.node.ble_hello().expect("identity");
    assert!(a.node.ble_verify(c.address.as_bytes(), &identity).is_err());
    assert!(a.node.ble_verify(&[0xff, 0xfe], &identity).is_err());
    let mut forged = identity.clone();
    let last = forged.len() - 1;
    forged[last] ^= 1;
    assert!(a.node.ble_verify(b.address.as_bytes(), &forged).is_err());
    let own = a.node.ble_hello().expect("own");
    let own_address = a.address.clone();
    assert!(
        a.node.ble_verify(own_address.as_bytes(), &own).is_err(),
        "this device's own identity, replayed, names no peer"
    );
}

#[test]
fn only_a_verified_address_is_linked_and_only_a_linked_one_delivers() {
    let (mut a, b) = (side("browser"), side("phone"));
    assert!(
        a.node.ble_link_up(&b.address, 244).is_err(),
        "a page must not announce an address the engine did not verify"
    );
    assert!(a.node.ble_fragment_received(&b.address, b"OP").is_err());
    assert!(a.node.ble_next_fragment().is_none());
}

#[test]
fn a_link_that_goes_down_reports_the_peer_lost_once() {
    let (mut a, mut b) = (side("browser"), side("phone"));
    link(&mut a, &mut b, 244);
    let b_addr = b.address.clone();
    pump(&mut a, &mut b);
    // A disconnect is seen at both ends; Alice's page reports it twice.
    let a_addr = a.address.clone();
    a.node.ble_link_down(&b_addr);
    a.node.ble_link_down(&b_addr);
    b.node.ble_link_down(&a_addr);
    pump(&mut a, &mut b);
    let lost = a
        .events
        .iter()
        .filter(|e| e["type"] == "neighbor_lost" && e["peer_id"] == b_addr.as_str())
        .count();
    assert_eq!(lost, 1);
    assert!(
        a.node.ble_fragment_received(&b_addr, b"OP").is_err(),
        "a link that went down delivers nothing more"
    );
}

#[test]
fn a_node_without_bluetooth_has_no_slot() {
    let mut node = Node::new("web-ble-test", "plain").expect("node");
    node.initialize().expect("initialize");
    assert!(node.ble_verify(b"x", b"y").is_err());
    assert!(node.ble_next_fragment().is_none());
}

/// Collects the events a side has queued.
fn drain(s: &mut Side) {
    while let Some(json) = s.node.poll_event() {
        s.events
            .push(serde_json::from_str(&json).expect("event json"));
    }
}

fn count(s: &Side, kind: &str, peer: &str) -> usize {
    s.events
        .iter()
        .filter(|e| e["type"] == kind && e["peer_id"] == peer)
        .count()
}

/// Verifies `peripheral` and links it on `central` only, as a page does
/// when it opens a link to a device.
fn central_link(central: &mut Side, peripheral: &Side, payload: u32) -> Result<(), String> {
    let identity = peripheral.node.ble_hello().expect("identity");
    let proved = central
        .node
        .ble_verify(peripheral.address.as_bytes(), &identity)
        .map_err(|e| e.to_string())?;
    central
        .node
        .ble_link_up(&proved, payload)
        .map_err(|e| e.to_string())
}

/// Opens a peer stream between the two, as a tab or a data channel would,
/// and moves its preambles across, so each announces the other on it.
fn stream(a: &mut Side, b: &mut Side) -> (LinkId, LinkId) {
    let (la, lb) = (
        a.node.open_link().expect("link"),
        b.node.open_link().expect("link"),
    );
    for _ in 0..4 {
        while let Some((link, data)) = a.node.next_outbound() {
            assert_eq!(link, la);
            assert_eq!(b.node.link_received(lb, &data), None);
        }
        while let Some((link, data)) = b.node.next_outbound() {
            assert_eq!(link, lb);
            assert_eq!(a.node.link_received(la, &data), None);
        }
    }
    (la, lb)
}

#[test]
fn a_replaced_link_to_the_same_address_is_not_announced_again() {
    let (mut a, b) = (side("browser"), side("phone"));
    let b_addr = b.address.clone();
    central_link(&mut a, &b, 244).expect("first link");
    central_link(&mut a, &b, 300).expect("the page replaced its link");
    drain(&mut a);
    assert_eq!(count(&a, "neighbor_discovered", &b_addr), 1);
    assert_eq!(count(&a, "neighbor_lost", &b_addr), 0);
    a.node.ble_link_down(&b_addr);
    drain(&mut a);
    assert_eq!(count(&a, "neighbor_lost", &b_addr), 1);
}

#[test]
fn a_peer_also_linked_over_a_stream_is_lost_only_with_its_last_link() {
    // Bluetooth goes first.
    let (mut a, mut b) = (side("browser"), side("phone"));
    let b_addr = b.address.clone();
    central_link(&mut a, &b, 244).expect("link");
    let (la, _) = stream(&mut a, &mut b);
    drain(&mut a);
    a.node.ble_link_down(&b_addr);
    drain(&mut a);
    assert_eq!(
        count(&a, "neighbor_lost", &b_addr),
        0,
        "the stream still reaches the peer"
    );
    a.node.close_link(la);
    drain(&mut a);
    assert_eq!(count(&a, "neighbor_lost", &b_addr), 1);

    // The stream goes first.
    let (mut c, mut d) = (side("browser-two"), side("phone-two"));
    let d_addr = d.address.clone();
    let (lc, _) = stream(&mut c, &mut d);
    central_link(&mut c, &d, 244).expect("link");
    drain(&mut c);
    c.node.close_link(lc);
    drain(&mut c);
    assert_eq!(
        count(&c, "neighbor_lost", &d_addr),
        0,
        "Bluetooth still reaches the peer"
    );
    c.node.ble_link_down(&d_addr);
    drain(&mut c);
    assert_eq!(count(&c, "neighbor_lost", &d_addr), 1);
}

#[test]
fn links_past_the_ceiling_are_refused() {
    let mut a = side("browser");
    let devices: Vec<Side> = (0..=MAX_LINKS)
        .map(|n| side(&format!("phone-{n}")))
        .collect();
    for device in &devices[..MAX_LINKS] {
        central_link(&mut a, device, 244).expect("under the ceiling");
    }
    let refused = central_link(&mut a, &devices[MAX_LINKS], 244).expect_err("over the ceiling");
    assert!(refused.contains("already open"), "{refused}");
    a.node.ble_link_down(&devices[0].address);
    central_link(&mut a, &devices[MAX_LINKS], 244).expect("a slot freed");
}

#[test]
fn a_page_that_only_verifies_holds_a_bounded_list() {
    let mut a = side("browser");
    let devices: Vec<Side> = (0..=16).map(|n| side(&format!("phone-{n}"))).collect();
    for device in &devices {
        let identity = device.node.ble_hello().expect("identity");
        a.node
            .ble_verify(device.address.as_bytes(), &identity)
            .expect("verify");
    }
    assert!(
        a.node.ble_link_up(&devices[0].address, 244).is_err(),
        "the oldest verification was evicted"
    );
    a.node
        .ble_link_up(&devices[16].address, 244)
        .expect("the newest is held");
}

#[test]
fn a_blocked_peer_is_linked_but_not_announced() {
    let (mut a, b) = (side("browser"), side("phone"));
    let b_addr = b.address.clone();
    a.node.protocol.block_user(&b_addr).expect("block");
    central_link(&mut a, &b, 244).expect("link");
    drain(&mut a);
    assert_eq!(count(&a, "neighbor_discovered", &b_addr), 0);
}

#[test]
fn stopping_reports_every_bluetooth_peer_lost() {
    let (mut a, b) = (side("browser"), side("phone"));
    let b_addr = b.address.clone();
    central_link(&mut a, &b, 244).expect("link");
    a.node.stop().expect("stop");
    drain(&mut a);
    assert_eq!(count(&a, "neighbor_lost", &b_addr), 1);
    assert!(a.node.ble_fragment_received(&b_addr, b"OP").is_err());
}

/// Moves every queued fragment among `sides` to the side it names, then
/// ticks each, so any number of nodes share one simulated radio. A side at
/// `broadcaster` reaches every other side with each fragment, whoever it
/// names, as a BlueZ peripheral does: it has no per-central notification.
fn pump_all(sides: &mut [&mut Side], broadcaster: Option<usize>) {
    let addresses: Vec<String> = sides.iter().map(|s| s.address.clone()).collect();
    let mut moved = Vec::new();
    for (from, s) in sides.iter_mut().enumerate() {
        while let Some((peer, fragment)) = s.node.ble_next_fragment() {
            moved.push((from, peer, fragment));
        }
    }
    for (from, peer, fragment) in moved {
        let to: Vec<usize> = if broadcaster == Some(from) {
            (0..sides.len()).filter(|&i| i != from).collect()
        } else {
            let named = addresses.iter().position(|a| *a == peer);
            vec![named.expect("a fragment for a node on the radio")]
        };
        for to in to {
            sides[to]
                .node
                .ble_fragment_received(&addresses[from], &fragment)
                .expect("fragment");
        }
    }
    for s in sides.iter_mut() {
        s.node.process().expect("process");
        drain(s);
    }
}

fn run_all(
    sides: &mut [&mut Side],
    broadcaster: Option<usize>,
    mut done: impl FnMut(&mut [&mut Side]) -> bool,
) -> bool {
    for _ in 0..800 {
        pump_all(sides, broadcaster);
        if done(sides) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn received(s: &Side, content: &str) -> bool {
    s.events.iter().any(|e| {
        e["type"] == "message_received" && e["content"] == content && e["encrypted"] == true
    })
}

/// Two pages that can never link to each other (a browser is a central
/// only) reach each other through the one peripheral both are linked to.
/// No discovery names a peer two hops away, so this rests on the tick's
/// mesh push of the key package and the Welcome.
#[test]
fn two_browsers_reach_each_other_through_one_peripheral() {
    let (mut a, mut hub, mut c) = (side("browser-a"), side("phone"), side("browser-c"));
    link(&mut a, &mut hub, 244);
    link(&mut c, &mut hub, 244);
    let (a_addr, c_addr) = (a.address.clone(), c.address.clone());

    a.node.send_message(&c_addr, "a to c", false).expect("send");
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], None, |s| received(
            s[2], "a to c"
        )),
        "c never read a's message through the hub"
    );
    c.node.send_message(&a_addr, "c to a", false).expect("send");
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], None, |s| received(
            s[0], "c to a"
        )),
        "a never read c's reply through the hub"
    );
    assert_eq!(count(&a, "neighbor_discovered", &c_addr), 0);
    assert!(count(&a, "secure_session_established", &c_addr) > 0);
    assert!(!hub
        .events
        .iter()
        .any(|e| e["type"] == "message_received" && e["content"] == "a to c"));

    // A group of the two browsers, the hub not in it, carried by the hub.
    let group: Value =
        serde_json::from_str(&a.node.create_group("over the hub").expect("group")).expect("json");
    let group_id = group["group_id"].as_str().expect("group id").to_string();
    // The invite needs the key package c sends after the session forms,
    // which crosses the hub a tick or two behind it.
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], None, |s| s[0]
            .node
            .invite_to_group(&group_id, &c_addr)
            .is_ok()),
        "c's key package never crossed the hub"
    );
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], None, |s| s[2]
            .events
            .iter()
            .any(
                |e| e["type"] == "group_member_added" && e["group_id"] == group_id.as_str()
            )),
        "c never joined a's group through the hub"
    );
    c.node
        .send_group_message(&group_id, "to the group", false)
        .expect("group send");
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], None, |s| s[0]
            .events
            .iter()
            .any(
                |e| e["type"] == "group_message_received" && e["content"] == "to the group"
            )),
        "a never read c's group message through the hub"
    );
}

#[test]
fn a_message_sent_before_the_other_browser_arrives_crosses_the_hub_later() {
    let (mut a, mut hub, mut c) = (side("browser-a"), side("phone"), side("browser-c"));
    link(&mut a, &mut hub, 244);
    let c_addr = c.address.clone();
    a.node.send_message(&c_addr, "early", false).expect("send");
    // Two seconds with c out of range.
    for _ in 0..80 {
        pump_all(&mut [&mut a, &mut hub], None);
        std::thread::sleep(Duration::from_millis(25));
    }
    link(&mut c, &mut hub, 244);
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], None, |s| received(
            s[2], "early"
        )),
        "a message queued while c was away never reached it"
    );
}

/// Each page behind a BlueZ peripheral also receives the other's frames.
/// It cannot decrypt them, so they cost airtime and nothing else.
#[test]
fn two_browsers_reach_each_other_through_a_peripheral_that_notifies_everyone() {
    let (mut a, mut hub, mut c) = (side("browser-a"), side("pi"), side("browser-c"));
    link(&mut a, &mut hub, 244);
    link(&mut c, &mut hub, 244);
    let (a_addr, c_addr) = (a.address.clone(), c.address.clone());
    a.node.send_message(&c_addr, "a to c", false).expect("send");
    c.node.send_message(&a_addr, "c to a", false).expect("send");
    assert!(
        run_all(&mut [&mut a, &mut hub, &mut c], Some(1), |s| received(
            s[2], "a to c"
        ) && received(
            s[0], "c to a"
        )),
        "a broadcasting peripheral stopped delivery"
    );
}
