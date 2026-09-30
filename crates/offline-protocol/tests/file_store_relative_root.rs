//! The built-in file stores open over a relative root on the first try.
//!
//! A store creates its directories and flushes each one's parent so the new
//! entry survives a crash. For a one-component relative root such as `keys`,
//! `Path::parent` is the empty path, and flushing that fails with "not
//! found": the first open failed and the second, finding the directory
//! there, succeeded. Every unit test opens an absolute temporary root, so
//! none of them reaches it.
//!
//! This is its own test binary because it changes the process's working
//! directory, which every other test in a shared binary would see.

#![cfg(feature = "file-store")]

use offline_protocol::file_store::{account_storage_namespace, FileStorePair, StaticStoreKey};

#[test]
fn a_pair_opens_over_one_component_relative_roots_on_the_first_try() {
    let base = std::env::temp_dir().join(format!(
        "offline-protocol-relative-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("create the working directory");
    let previous = std::env::current_dir().expect("current directory");
    std::env::set_current_dir(&base).expect("enter the working directory");

    let namespace = account_storage_namespace("com.example.relative", "alice");
    let outcome = FileStorePair::open("keys", "state", &namespace, &StaticStoreKey::new([7; 32]));
    let created = (base.join("keys").is_dir(), base.join("state").is_dir());

    std::env::set_current_dir(&previous).expect("leave the working directory");
    let pair = outcome.expect("the first open over relative roots");
    pair.close();
    let _ = std::fs::remove_dir_all(&base);

    assert_eq!(
        created,
        (true, true),
        "both roots are made under the working directory"
    );
}
