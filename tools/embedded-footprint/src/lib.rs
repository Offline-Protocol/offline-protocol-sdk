//! Bare-metal runtime shared by the two footprint binaries.
//!
//! Nothing here is firmware and nothing here ever runs. Both binaries exist to
//! be linked and measured; the allocator below allocates and never frees,
//! which is fine for a program that is never executed and wrong for anything
//! that is. A real leaf node needs a real allocator, which costs roughly one
//! to two more kilobytes of flash than this.

#![no_std]

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Arbitrary, and it cancels out. The array lands in `.bss` identically in
/// both binaries, so it contributes nothing to the baseline-to-protocol delta
/// that the report quotes.
const HEAP_SIZE: usize = 16 * 1024;

static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Bump;

// SAFETY: `HEAP` is only ever reached through the atomically-claimed offset
// below, so two callers never receive overlapping regions. Cortex-M33
// implements the exclusive-access instructions `AtomicUsize` compiles to.
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = &raw mut HEAP as *mut u8;
        let mut cursor = NEXT.load(Ordering::Relaxed);
        loop {
            let start = (cursor + layout.align() - 1) & !(layout.align() - 1);
            let end = match start.checked_add(layout.size()) {
                Some(end) if end <= HEAP_SIZE => end,
                _ => return ptr::null_mut(),
            };
            match NEXT.compare_exchange_weak(cursor, end, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return base.add(start),
                Err(observed) => cursor = observed,
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: Bump = Bump;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

/// Entropy, for linking only.
///
/// `rand_core::OsRng` reaches `getrandom`, which has no backend on a bare-metal
/// target, so the link fails without one registered. This counter is **not**
/// entropy and must never be copied into firmware: a real leaf node wires this
/// symbol to its vendor TRNG, and MLS key generation is only as strong as what
/// it returns. It is acceptable here for exactly one reason, which is the same
/// reason the allocator above never frees: this image is linked and measured,
/// never executed.
#[cfg(feature = "leaf-base")]
mod rng {
    use core::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn fill(dest: &mut [u8]) -> Result<(), getrandom::Error> {
        for byte in dest.iter_mut() {
            *byte = COUNTER.fetch_add(1, Ordering::Relaxed) as u8;
        }
        Ok(())
    }

    getrandom::register_custom_getrandom!(fill);
}
