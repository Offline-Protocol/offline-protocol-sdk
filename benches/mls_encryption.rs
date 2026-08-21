//! MLS seal and open cost per 1:1 message.
//!
//! `send_message` in protocol_performance.rs deliberately opts out of
//! encryption (SEC-M3 fail-closed would otherwise reject the send), so its
//! figure is the dispatch path only. These benches measure the term it
//! excludes: sealing a DM into an MLS ciphertext over an established session,
//! and opening one on the receiving side.
//!
//! The plaintext is the same 5-byte `hello` the wire-size record uses, so the
//! size table and the cost table describe one message. The open bench must be
//! fed a fresh ciphertext per iteration: the ratchet refuses replays, so a
//! reused ciphertext would measure an error return, which is the exact trap
//! `send_message` fell into once already (see the #391 history before trusting
//! a suspiciously fast number here).
//!
//! Storage is the in-memory test store. On device the session-state writes
//! land in a platform keystore instead, so treat these as the crypto cost,
//! not the crypto-plus-persistence cost.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use offline_protocol::mls::InMemoryStorage;
use offline_protocol::{MlsManager, MlsStorage};
use std::hint::black_box;
use std::sync::Arc;

/// Two managers with a live 1:1 session, built the way production does it:
/// identity first, manager at the derived address, key-package import,
/// session create, Welcome join.
fn session_pair() -> (MlsManager, String, MlsManager, String) {
    let mk = || {
        let storage: Arc<dyn MlsStorage> = Arc::new(InMemoryStorage::new());
        let (_keys, address) =
            MlsManager::load_or_create_identity(&storage).expect("mint identity");
        let manager = MlsManager::new(address.to_string(), storage).expect("construct manager");
        (manager, address.to_string())
    };
    let (alice, alice_addr) = mk();
    let (bob, bob_addr) = mk();
    let kp = bob.generate_key_package().expect("bob key package");
    alice
        .import_key_package(&bob_addr, &kp.key_package_data)
        .expect("import bob's key package");
    let welcome = alice.create_session(&bob_addr).expect("create session");
    bob.join_session(&welcome).expect("join session");
    (alice, alice_addr, bob, bob_addr)
}

fn bench_mls_seal(c: &mut Criterion) {
    let (alice, _alice_addr, _bob, bob_addr) = session_pair();
    c.bench_function("mls_seal_dm", |b| {
        b.iter(|| {
            let ct = alice
                .encrypt_for_existing_session(&bob_addr, black_box(b"hello"))
                .expect("seal must succeed; an Err here means the bench is timing a rejection");
            black_box(ct)
        })
    });
}

fn bench_mls_open(c: &mut Criterion) {
    let (alice, alice_addr, bob, bob_addr) = session_pair();
    c.bench_function("mls_open_dm", |b| {
        b.iter_batched(
            || {
                alice
                    .encrypt_for_existing_session(&bob_addr, b"hello")
                    .expect("seal fresh ciphertext")
            },
            |ct| {
                let pt = bob
                    .decrypt_from_user(&ct, &alice_addr)
                    .expect("open must succeed; an Err here means the bench is timing a rejection");
                black_box(pt)
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_mls_seal, bench_mls_open);
criterion_main!(benches);
