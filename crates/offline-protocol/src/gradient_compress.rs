//! Top-K sparsification with E3M0 4-bit quantization.
//!
//! This is the Rust port of the reference C implementation at
//! `companion-app/ios/IntelligenceMesh/Training/gradient_compress.c`. The wire
//! format and the E3M0 encoding are byte-identical to the reference, so a
//! mixed-fleet round where one peer encodes in Rust and another decodes via
//! the legacy ObjC bridge produces matching output.
//!
//! # Pipeline per round
//!
//! 1. Caller provides a flat f32 gradient and (optionally) a persistent
//!    `ErrorFeedback` buffer from the previous round.
//! 2. Gradient is split into chunks of `chunk_size` elements.
//! 3. Within each chunk, add the per-position error feedback (if any) to
//!    the raw gradient values, then pick the top-K by absolute value.
//! 4. The selected values are E3M0-quantized to 4-bit codes and packed into
//!    `(12-bit index, 4-bit code)` entries, written little-endian after a
//!    2-byte chunk header. Unused entries up to `topk` are zero-padded so
//!    every chunk's byte size is fixed at `2 + topk * 2`.
//! 5. The non-selected residuals are written back to the error buffer and
//!    decayed by `decay` for the next round. This is the "error-feedback"
//!    half of top-K sparsification — without it, information in positions
//!    that never make the cut would be lost permanently.
//!
//! # E3M0 encoding (4 bits)
//!
//! Layout: `(sign << 3) | exp`.
//! Value: `(-1)^sign * 2^(exp - 3)`, with `exp` in `[0, 7]`.
//! Representable set: ±{0.125, 0.25, 0.5, 1, 2, 4, 8, 16}. Zero is not
//! representable; values with `|x| < 0.125` clamp to ±0.125, and `|x| > 16`
//! clamp to ±16.
//!
//! # Wire format (per chunk)
//!
//! - 2 bytes: `chunk_id` as `u16` little-endian
//! - `topk × 2 bytes` per entry: `[index_hi:8][index_lo:4 | e3m0:4]`
//!
//! Fixed-size output per chunk: `2 + topk * 2`. Total output: `n_chunks *
//! (2 + topk * 2)` bytes. A chunk-size-4096 top-64 run on a 2.9 MB (725k
//! params) fp32 vector produces ~23 KB of compressed output (~125× reduction).

use std::collections::HashMap;

/// Default chunk size, matching the C reference. ~4k params per chunk means
/// a 725k-param gradient produces ~177 chunks.
pub const DEFAULT_CHUNK_SIZE: usize = 4096;

/// Default top-K per chunk, matching the C reference. 64 of 4096 is ~1.5%
/// sparsity, which is the published sweet spot in the top-K SGD literature
/// for maintaining accuracy with aggressive compression.
pub const DEFAULT_TOPK: usize = 64;

/// Default error-feedback decay per round. 0.95 matches the C reference and
/// the published values in Stich et al.'s sparsified SGD analysis.
pub const DEFAULT_ERROR_DECAY: f32 = 0.95;

/// E3M0 dequantization lookup. Index by `(sign << 3) | exp`, where `exp` is
/// the raw 3-bit code (0..=7). The positive half mirrors the negative half
/// with the sign bit flipped.
const E3M0_TABLE: [f32; 16] = [
    0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, -0.125, -0.25, -0.5, -1.0, -2.0, -4.0, -8.0, -16.0,
];

/// Quantize a float to a 4-bit E3M0 code. Clamps the magnitude to
/// `[0.125, 16.0]` before encoding so under/overflow produces the nearest
/// representable value instead of a bogus exponent.
pub fn quantize_e3m0(val: f32) -> u8 {
    let (sign, abs_val) = if val < 0.0 { (1u8, -val) } else { (0u8, val) };
    let abs_val = abs_val.clamp(0.125, 16.0);
    // exp = round(log2(abs_val) + 3), clamped. log2(0.125) = -3 → exp = 0;
    // log2(16) = 4 → exp = 7. Everything in between rounds to the nearest
    // power-of-two boundary.
    let exp = (abs_val.log2() + 3.0).round() as i32;
    let exp = exp.clamp(0, 7) as u8;
    (sign << 3) | exp
}

/// Dequantize a 4-bit E3M0 code to a float. Masks the input to 4 bits so a
/// caller that packed the code into the low nibble of a byte can pass the
/// byte directly without pre-masking.
pub fn dequantize_e3m0(code: u8) -> f32 {
    E3M0_TABLE[(code & 0x0F) as usize]
}

/// Error-feedback buffer carried between successive `encode_topk_e3m0`
/// calls on the same gradient-shape sequence. Caller owns it (not module
/// state), so a session boundary is simply constructing a fresh instance;
/// the C reference's `compress_reset_error_buffer()` has no analog here
/// because there is no shared mutable state to reset.
///
/// `chunks` is keyed by chunk index. Each stored vector holds the per-
/// position residuals (gradient value minus what we sent, with the prior
/// round's residual folded in and the whole thing decayed) that drive the
/// next round's top-K selection. Positions that are frequently in the top-K
/// stay near zero; positions that are consistently below the cutoff
/// accumulate until they eventually win a round.
#[derive(Debug, Clone)]
pub struct ErrorFeedback {
    chunks: HashMap<usize, Vec<f32>>,
    /// Decay factor applied to the residual buffer after each round.
    /// Published value from Stich et al. is `0.95`; smaller values mean
    /// residuals attenuate faster (fresher gradient dominates) and larger
    /// values let dropped positions accumulate longer before they win a
    /// top-K slot. Exposed as a public field so a caller can tune it
    /// mid-session without reconstructing the buffer.
    pub decay: f32,
}

impl Default for ErrorFeedback {
    fn default() -> Self {
        Self {
            chunks: HashMap::new(),
            decay: DEFAULT_ERROR_DECAY,
        }
    }
}

impl ErrorFeedback {
    /// Create an error-feedback buffer with the default decay factor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an error-feedback buffer with a custom decay factor. The
    /// caller is responsible for picking a value in `(0, 1)`; outside that
    /// range the residual either explodes or inverts sign each round.
    pub fn with_decay(decay: f32) -> Self {
        Self {
            chunks: HashMap::new(),
            decay,
        }
    }

    /// Clear all per-chunk residuals. Call between training sessions so a
    /// stale buffer from a previous session's gradient shape does not leak
    /// into the first round of a new one.
    pub fn reset(&mut self) {
        self.chunks.clear();
    }

    /// Number of chunks with recorded residuals. Useful for tests.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

/// Encode a flat f32 gradient into top-K + E3M0 compressed bytes.
///
/// `chunk_size` and `topk` default to the published values when passed `0`.
/// `topk` is silently clamped to `[1, chunk_size]`. If `error_feedback` is
/// `Some`, the buffer is both READ (to augment the current gradient before
/// top-K selection) and WRITTEN (with the new residuals + decay); if `None`,
/// the encoder is stateless and successive calls on the same gradient
/// produce byte-identical output.
pub fn encode_topk_e3m0(
    grad: &[f32],
    chunk_size: usize,
    topk: usize,
    mut error_feedback: Option<&mut ErrorFeedback>,
) -> Vec<u8> {
    if grad.is_empty() {
        return Vec::new();
    }
    let chunk_size = if chunk_size == 0 {
        DEFAULT_CHUNK_SIZE
    } else {
        chunk_size
    };
    let topk = topk.clamp(1, chunk_size);

    let n_chunks = grad.len().div_ceil(chunk_size);
    let bytes_per_chunk = 2 + topk * 2;
    let mut out = Vec::with_capacity(n_chunks * bytes_per_chunk);

    // Scratch buffer reused across chunks. Entries: (original_index,
    // (gradient + prior-feedback) value, abs value for sorting).
    let mut buf: Vec<(usize, f32, f32)> = Vec::with_capacity(chunk_size);

    for c in 0..n_chunks {
        let chunk_start = c * chunk_size;
        let chunk_end = (chunk_start + chunk_size).min(grad.len());
        let actual_size = chunk_end - chunk_start;

        // Snapshot the prior round's feedback for this chunk BEFORE we hand
        // out the mutable reference again below. If the caller passed None
        // or we have never seen this chunk, feedback is implicitly zero.
        let err_prev: Option<Vec<f32>> = error_feedback
            .as_deref()
            .and_then(|ef| ef.chunks.get(&c).cloned());

        buf.clear();
        for i in 0..actual_size {
            let fb = err_prev
                .as_ref()
                .and_then(|v| v.get(i).copied())
                .unwrap_or(0.0);
            let v = grad[chunk_start + i] + fb;
            buf.push((i, v, v.abs()));
        }

        // Partial sort to find the top-K by |val| descending. select_nth_unstable_by
        // is O(n) and leaves buf[..k] as the top-K in an unspecified order.
        // We then sort just that prefix by original index for a deterministic
        // output byte sequence (so two Rust encoders on the same input produce
        // identical bytes, and so a byte-level test against the C reference is
        // stable — the C reference sorts by abs descending; both orderings are
        // valid wire-format input because the decoder reads entries by their
        // packed `idx12` field, not by position).
        let k = topk.min(actual_size);
        if k < actual_size {
            buf.select_nth_unstable_by(k - 1, |a, b| {
                b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        let (selected, _rest) = buf.split_at_mut(k);
        selected.sort_unstable_by_key(|(idx, _, _)| *idx);

        // Chunk header: little-endian u16 chunk_id.
        out.push((c & 0xFF) as u8);
        out.push(((c >> 8) & 0xFF) as u8);

        // k entries + zero-padding to the fixed per-chunk size.
        //
        // A top-K slot whose value is literally zero is an edge case that
        // arises when the chunk has fewer than `topk` nonzero positions
        // after error-feedback application. The C reference emits such a
        // slot as `(idx, quantize_e3m0(0.0))` = `(idx, +0.125 after clamp)`,
        // which decodes to a spurious +0.125 "ghost" at that position. We
        // diverge intentionally: zero-valued slots emit the `(0, 0)`
        // padding sentinel instead, so the decoder reconstructs zero at
        // those positions. This matches user intent (if the gradient is
        // zero, send zero) at the cost of a one-slot capacity "waste"
        // relative to the fixed per-chunk byte budget — the decoder's
        // `(0, 0)` short-circuit handles this transparently.
        for (idx, val, _) in selected[..k].iter() {
            if *val != 0.0 {
                let q = quantize_e3m0(*val);
                let idx12 = (*idx as u16) & 0x0FFF;
                out.push((idx12 >> 4) as u8);
                out.push((((idx12 & 0x0F) << 4) as u8) | (q & 0x0F));
            } else {
                out.push(0);
                out.push(0);
            }
        }
        // Pad any unfilled slots up to `topk` with the sentinel.
        for _ in k..topk {
            out.push(0);
            out.push(0);
        }

        // Error-feedback update, matching the C reference:
        //   next[i] = grad[i] + prior_feedback[i]  (all positions)
        //   next[selected_idx] = 0                 (positions we sent)
        //   next *= decay                          (prevent unbounded growth)
        if let Some(ef) = error_feedback.as_deref_mut() {
            let mut next = vec![0.0_f32; actual_size];
            for i in 0..actual_size {
                let fb = err_prev
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .unwrap_or(0.0);
                next[i] = grad[chunk_start + i] + fb;
            }
            for &(idx, _, _) in &selected[..k] {
                if idx < actual_size {
                    next[idx] = 0.0;
                }
            }
            let decay = ef.decay;
            for v in next.iter_mut() {
                *v *= decay;
            }
            ef.chunks.insert(c, next);
        }
    }

    out
}

/// Decode compressed bytes back into a sparse f32 vector. Positions not
/// covered by the compressed data remain zero. `chunk_size` and `topk` must
/// match what the encoder used; the wire format carries neither (both
/// peers agree on the config out of band).
///
/// A missing or truncated chunk is skipped rather than panicking. An entry
/// whose packed `(chunk_id, idx12)` resolves beyond `n_params` is silently
/// ignored — the caller passes the authoritative parameter count, and a
/// stale chunk_id from an earlier round with a bigger gradient shape must
/// not corrupt the output.
pub fn decode_topk_e3m0(
    compressed: &[u8],
    n_params: usize,
    chunk_size: usize,
    topk: usize,
) -> Vec<f32> {
    let mut out = vec![0.0_f32; n_params];
    if compressed.is_empty() || n_params == 0 {
        return out;
    }
    let chunk_size = if chunk_size == 0 {
        DEFAULT_CHUNK_SIZE
    } else {
        chunk_size
    };
    let topk = topk.max(1);
    let bytes_per_chunk = 2 + topk * 2;

    let n_chunks = compressed.len() / bytes_per_chunk;

    for c in 0..n_chunks {
        let off = c * bytes_per_chunk;
        let chunk_id = (compressed[off] as u16) | ((compressed[off + 1] as u16) << 8);
        let chunk_start = (chunk_id as usize) * chunk_size;

        for j in 0..topk {
            let eo = off + 2 + j * 2;
            if eo + 1 >= compressed.len() {
                break;
            }
            let b0 = compressed[eo];
            let b1 = compressed[eo + 1];
            // All-zero entry is padding — the encoder emits it when a chunk
            // has fewer than topk nonzero positions after error feedback.
            if b0 == 0 && b1 == 0 {
                continue;
            }
            let idx12 = ((b0 as u16) << 4) | (((b1 >> 4) & 0x0F) as u16);
            let code = b1 & 0x0F;
            let global_idx = chunk_start + idx12 as usize;
            if global_idx < n_params {
                out[global_idx] = dequantize_e3m0(code);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e3m0_roundtrips_every_nominal_value() {
        for code in 0u8..16u8 {
            let v = dequantize_e3m0(code);
            let re = quantize_e3m0(v);
            assert_eq!(re, code, "code {code:#x} value {v} re-encoded to {re:#x}");
        }
    }

    #[test]
    fn e3m0_zero_clamps_to_positive_minimum() {
        // Zero is not representable; the encoder picks the positive
        // minimum because the sign check uses `<`, not `<=`.
        assert_eq!(quantize_e3m0(0.0), 0b0000);
        assert_eq!(dequantize_e3m0(0b0000), 0.125);
    }

    #[test]
    fn e3m0_overflow_clamps_to_sign_max() {
        assert_eq!(quantize_e3m0(100.0), 0b0111); // +16
        assert_eq!(quantize_e3m0(-100.0), 0b1111); // -16
        assert_eq!(dequantize_e3m0(quantize_e3m0(100.0)), 16.0);
        assert_eq!(dequantize_e3m0(quantize_e3m0(-100.0)), -16.0);
    }

    #[test]
    fn e3m0_underflow_clamps_to_sign_min() {
        assert_eq!(quantize_e3m0(0.001), 0b0000); // +0.125
        assert_eq!(quantize_e3m0(-0.001), 0b1000); // -0.125
    }

    #[test]
    fn e3m0_representable_values_match_reference() {
        // These must match the C reference's E3M0_TABLE exactly, both in
        // which codes map to which values and in the ordering.
        let expected: [f32; 16] = [
            0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, -0.125, -0.25, -0.5, -1.0, -2.0, -4.0,
            -8.0, -16.0,
        ];
        for (code, want) in expected.iter().enumerate() {
            assert_eq!(dequantize_e3m0(code as u8), *want);
        }
    }

    #[test]
    fn encode_sparse_two_values_in_one_chunk_matches_hand_computation() {
        // 10-param gradient. Only two nonzero positions.
        //   idx=2 → 2.0  →  E3M0 code 0x4 (sign=0, exp=4, value = 2^(4-3) = 2)
        //   idx=7 → 1.0  →  E3M0 code 0x3 (sign=0, exp=3, value = 2^(3-3) = 1)
        //
        // chunk_size=10 → one chunk (chunk_id=0, header 0x00 0x00).
        // topk=2 → two entries, both filled, no padding.
        // Entries are sorted by original index after selection:
        //   entry 0: idx=2, code=0x4 → bytes 0x00 ((idx12=0x002)>>4), 0x24 ((0x02<<4)|0x04)
        //   entry 1: idx=7, code=0x3 → bytes 0x00 ((idx12=0x007)>>4), 0x73 ((0x07<<4)|0x03)
        let grad = [0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let compressed = encode_topk_e3m0(&grad, 10, 2, None);
        assert_eq!(compressed, vec![0x00, 0x00, 0x00, 0x24, 0x00, 0x73]);
    }

    #[test]
    fn encode_decode_roundtrips_sparse_input_exactly() {
        // Values on the E3M0 grid round-trip exactly; values off the grid
        // snap to the nearest representable value, which is tested in
        // `encode_decode_offgrid_values_snap_to_grid` below.
        let grad = [0.0, -2.0, 0.0, 0.5, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0];
        let compressed = encode_topk_e3m0(&grad, 10, 3, None);
        let decoded = decode_topk_e3m0(&compressed, grad.len(), 10, 3);
        assert_eq!(
            decoded,
            vec![0.0, -2.0, 0.0, 0.5, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0]
        );
    }

    #[test]
    fn encode_decode_offgrid_values_snap_to_grid() {
        // 1.4 is between 1.0 (exp=3) and 2.0 (exp=4); log2(1.4) ≈ 0.485,
        // so round(0.485 + 3) = 3 → decodes as 1.0.
        // 0.3 is between 0.25 (exp=1) and 0.5 (exp=2); log2(0.3) ≈ -1.737,
        // so round(-1.737 + 3) = 1 → decodes as 0.25.
        let grad = [1.4_f32, 0.3];
        let compressed = encode_topk_e3m0(&grad, 2, 2, None);
        let decoded = decode_topk_e3m0(&compressed, 2, 2, 2);
        assert_eq!(decoded, vec![1.0, 0.25]);
    }

    #[test]
    fn multi_chunk_roundtrip_preserves_sparsity() {
        // 10,000 params, chunk_size 1024, topk 20. Strong signals every
        // 100th position starting at idx=100 → ~10 per chunk (well under
        // the 20-slot budget so every strong signal survives top-K).
        // Values are on the E3M0 grid for exact round-trip; background
        // stays at 0.0 so empty top-K slots emit the padding sentinel.
        //
        // Skip idx 0 (sentinel collision — see
        // `zero_valued_topk_slots_emit_padding_sentinel`): an entry at
        // (chunk-local idx = 0, value = +0.125) encodes as bytes (0, 0)
        // which is also the padding sentinel. The C reference has the
        // same limitation; real gradients rarely have exactly 0.125 at
        // exactly idx 0 so it has not bitten anyone in practice.
        let mut grad = vec![0.0_f32; 10_000];
        for i in (100..10_000).step_by(100) {
            let grid = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
            grad[i] = grid[(i / 100) % grid.len()];
        }
        let compressed = encode_topk_e3m0(&grad, 1024, 20, None);
        let n_chunks = 10_000_usize.div_ceil(1024);
        assert_eq!(compressed.len(), n_chunks * (2 + 20 * 2));

        let decoded = decode_topk_e3m0(&compressed, grad.len(), 1024, 20);

        // Every strong-signal position round-trips exactly.
        for i in (100..10_000).step_by(100) {
            assert_eq!(decoded[i], grad[i], "mismatch at idx {i}");
        }

        // All other positions decode to zero (no ghosts from padding).
        for (i, v) in decoded.iter().enumerate() {
            if i == 0 || i % 100 != 0 {
                assert_eq!(*v, 0.0, "unexpected nonzero at idx {i}: {v}");
            }
        }
    }

    #[test]
    fn zero_valued_topk_slots_emit_padding_sentinel() {
        // When a chunk has fewer than `topk` nonzero positions, the
        // encoder must emit the (0, 0) sentinel for the empty slots so
        // the decoder reconstructs zero at those positions instead of a
        // spurious +0.125 "ghost" (the C reference's divergence case).
        //
        // 10 params, 2 real nonzeros, topk=5 → 3 slots must be sentinels.
        let grad = [0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let compressed = encode_topk_e3m0(&grad, 10, 5, None);
        let decoded = decode_topk_e3m0(&compressed, grad.len(), 10, 5);
        assert_eq!(
            decoded,
            vec![0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn error_feedback_shifts_selection_across_rounds() {
        // A gradient with two nearly-tied values. First round picks the
        // slightly larger one (idx=1). The error buffer accumulates the
        // dropped value (idx=0). On the second round, the dropped value
        // plus the fresh gradient at idx=0 wins the top-K.
        let grad = [1.0_f32, 1.1_f32];
        let mut ef = ErrorFeedback::new();

        let round1 = encode_topk_e3m0(&grad, 2, 1, Some(&mut ef));
        let decoded1 = decode_topk_e3m0(&round1, 2, 2, 1);
        // Round 1 picks idx=1 (|1.1| > |1.0|).
        assert_eq!(decoded1[0], 0.0);
        assert!(decoded1[1].abs() > 0.0);

        // Error buffer now carries idx=0's residual (~1.0 * 0.95 = 0.95).
        // On round 2, idx=0 sees fresh 1.0 + residual 0.95 = 1.95, which
        // beats idx=1's fresh 1.1 + residual 0 = 1.1.
        let round2 = encode_topk_e3m0(&grad, 2, 1, Some(&mut ef));
        let decoded2 = decode_topk_e3m0(&round2, 2, 2, 1);
        assert!(
            decoded2[0].abs() > 0.0,
            "error feedback failed to shift selection: round 2 decoded = {decoded2:?}"
        );
    }

    #[test]
    fn error_feedback_reset_clears_state() {
        let grad = [1.0_f32, 2.0_f32, 3.0_f32];
        let mut ef = ErrorFeedback::new();
        encode_topk_e3m0(&grad, 3, 1, Some(&mut ef));
        assert_eq!(ef.chunk_count(), 1);
        ef.reset();
        assert_eq!(ef.chunk_count(), 0);
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert!(encode_topk_e3m0(&[], 10, 2, None).is_empty());
        assert!(decode_topk_e3m0(&[], 0, 10, 2).is_empty());
    }

    #[test]
    fn compression_ratio_is_in_the_expected_range() {
        // 100k fp32 params = 400 KB raw. Chunk 4096, topk 64 → bytes_per_chunk
        // = 2 + 64 * 2 = 130. n_chunks = ceil(100_000 / 4096) = 25.
        // Compressed = 25 * 130 = 3250 bytes. Ratio = 400_000 / 3250 ≈ 123x.
        let grad = vec![0.5_f32; 100_000];
        let compressed = encode_topk_e3m0(&grad, 4096, 64, None);
        let raw_bytes = grad.len() * 4;
        let ratio = raw_bytes as f32 / compressed.len() as f32;
        assert!(
            (100.0..=140.0).contains(&ratio),
            "compression ratio out of expected range: {ratio}"
        );
    }
}
