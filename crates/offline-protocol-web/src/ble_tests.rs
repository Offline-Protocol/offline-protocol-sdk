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

use crate::node::{Node, NodeOptions};

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
