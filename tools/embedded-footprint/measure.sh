#!/usr/bin/env bash
#
# Reports what the protocol layer, and then a whole leaf node, cost on a
# Cortex-M33, each as a delta against a baseline firmware that has the same
# runtime and none of the protocol.
#
# Requires: rustup target add thumbv8m.main-none-eabihf
#           rustup component add llvm-tools-preview
#
# Pass --core-only to skip the MLS images, which is the right thing to do when
# the MLS dependency tree cannot be fetched.

set -euo pipefail
cd "$(dirname "$0")"

CORE_ONLY=0
[[ "${1:-}" == "--core-only" ]] && CORE_ONLY=1

TARGET=thumbv8m.main-none-eabihf
OUT="target/${TARGET}/release"

LLVM_SIZE=$(find "$(rustc --print sysroot)" -name llvm-size -type f | head -1)
if [[ -z "${LLVM_SIZE}" ]]; then
    echo "llvm-size not found. Run: rustup component add llvm-tools-preview" >&2
    exit 1
fi
# Fatal for the same reason `llvm-size` is. This one is the only guard the leaf
# images have against silently hollowing out, and a run that skipped it would
# still print a table, which is the failure mode the guard exists to prevent.
# Both binaries ship in `llvm-tools-preview`, so needing one is needing both.
LLVM_NM=$(find "$(rustc --print sysroot)" -name llvm-nm -type f | head -1)
if [[ -z "${LLVM_NM}" ]]; then
    echo "llvm-nm not found. Run: rustup component add llvm-tools-preview" >&2
    exit 1
fi

# `--locked` because the point of this harness is a number that does not move
# for reasons unrelated to the code being measured.
cargo build --release --locked --quiet

# `llvm-size -A` prints one "section size addr" row per section.
section() {  # section <binary> <section-name>
    "${LLVM_SIZE}" -A "$1" | awk -v s="$2" '$1 == s { print $2; found=1 } END { if (!found) print 0 }'
}

flash() {  # flash <binary>
    echo $(( $(section "$1" .text) + $(section "$1" .rodata) ))
}

base_flash=$(flash "${OUT}/baseline")
prot_flash=$(flash "${OUT}/protocol")
base_bss=$(section "${OUT}/baseline" .bss)
prot_bss=$(section "${OUT}/protocol" .bss)

awk -v bf="${base_flash}" -v pf="${prot_flash}" -v fd="$(( prot_flash - base_flash ))" \
    -v bd="$(( prot_bss - base_bss ))" -v tgt="${TARGET}" 'BEGIN {
  k = 1024
  printf "| Measurement | Bytes | KiB |\n"
  printf "|---|--:|--:|\n"
  printf "| Baseline firmware (runtime, allocator, panic handler) | %'"'"'d | %.1f |\n", bf, bf/k
  printf "| With offline-protocol-core linked | %'"'"'d | %.1f |\n", pf, pf/k
  printf "| **Protocol layer, flash** | **%'"'"'d** | **%.1f** |\n", fd, fd/k
  printf "| Protocol layer, static RAM (`.bss`) | %'"'"'d | %.1f |\n", bd, bd/k
  printf "\nMeasured on %s, release profile (opt-level \"z\", LTO, panic=abort),\n", tgt
  printf "core built `--no-default-features`. Static RAM excludes the heap: the\n"
  printf "harness provisions 16 KiB in both binaries so it cancels here, and a\n"
  printf "real node sizes its own from its workload.\n"
}'

[[ "${CORE_ONLY}" == "1" ]] && exit 0

# The leaf images. Each variant is a separate link, so they are built and
# measured one at a time into the same output path.
echo
echo "### Leaf node: protocol layer plus MLS"
echo
printf "| Configuration | Flash | vs baseline | vs protocol only |\n"
printf "|---|--:|--:|--:|\n"

declare -a LEAF_ROWS=(
    "leaf:never-committing leaf, the shipping profile"
    "leaf-full:rfc_compliant, X.509 included (upper bound)"
)

leaf_measured=0
for row in "${LEAF_ROWS[@]}"; do
    feature="${row%%:*}"
    label="${row#*:}"
    cargo build --release --locked --quiet --features "${feature}"
    f=$(flash "${OUT}/leaf")

    # The guard the sample-honesty problem needs. If either half ever stops
    # being linked (a workload that optimises away, a dependency that silently
    # drops out), the flash number falls toward `protocol` and reads as an
    # improvement. A symbol count cannot be fooled that way.
    #
    # Both halves are counted, because they fall out independently. The leaf
    # crate is the one this harness exists to price: a workload that drifted
    # back onto mls-rs directly would link the envelope codec, the
    # control-frame signing and the address derivation no more than the
    # version this replaced did, drop tens of kilobytes, and pass an mls-rs
    # count the whole way.
    for probe in "mls_rs:MLS" "offline_protocol_leaf:the leaf crate"; do
        symbol="${probe%%:*}"
        what="${probe#*:}"
        symbols=$("${LLVM_NM}" "${OUT}/leaf" 2>/dev/null | grep -c "${symbol}" || true)
        if (( symbols < 50 )); then
            echo "FAIL: only ${symbols} ${symbol} symbols in the ${feature} image." >&2
            echo "The workload stopped linking ${what}; the number below is not a footprint." >&2
            exit 1
        fi
    done

    awk -v f="${f}" -v bf="${base_flash}" -v pf="${prot_flash}" -v l="${label}" 'BEGIN {
      k = 1024
      printf "| %s | %.1f KiB | %.1f KiB | %.1f KiB |\n", l, f/k, (f-bf)/k, (f-pf)/k
    }'
    leaf_measured=1
done

if (( leaf_measured )); then
    cat <<'NOTE'

The first row is the number that answers "does it fit": on a 1536 KiB xG24
that is the whole leaf image, protocol layer included, against a part that also
has to hold a radio stack and an application.

Two things this does not measure. Heap is the first: MLS group state is
allocated, not static, so `.bss` stays flat here and the working-set figure has
to come from running the thing, not linking it. Interoperability is the second:
these images are linked and never executed, and the frame handed to the device
is an ordinary text message rather than a Welcome, so this says nothing about
whether the stack talks to the phone's OpenMLS. That question has its own
harness, and so do the leaf crate's own tests.
NOTE
fi
