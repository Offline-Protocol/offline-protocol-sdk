//! Everything the protocol binary has except the protocol.
//!
//! Subtracting this from `protocol` is what isolates
//! `offline-protocol-core`'s own cost from the cost of the `cortex-m-rt`
//! vector table, the allocator, and the panic handler, all of which any
//! firmware pays whether or not it speaks this protocol.
//!
//! It allocates once, deliberately. Without a live allocation the linker drops
//! the heap array entirely, `.bss` reads zero here and 16 KB in `protocol`,
//! and the RAM delta reports the harness's own heap as though it were a cost
//! of the protocol.

#![no_std]
#![no_main]

extern crate alloc;

// Links the allocator and the panic handler.
use embedded_footprint as _;

use alloc::vec::Vec;
use core::hint::black_box;
use cortex_m_rt::entry;

#[entry]
fn main() -> ! {
    let mut v: Vec<u8> = Vec::new();
    v.push(black_box(1));
    black_box(&v);
    loop {}
}
