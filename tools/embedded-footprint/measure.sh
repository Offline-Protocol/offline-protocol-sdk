#!/usr/bin/env bash
#
# Builds both binaries and reports what offline-protocol-core costs on a
# Cortex-M33, as a delta against a baseline firmware that has the same runtime
# and none of the protocol.
#
# Requires: rustup target add thumbv8m.main-none-eabihf
#           rustup component add llvm-tools-preview

set -euo pipefail
cd "$(dirname "$0")"

TARGET=thumbv8m.main-none-eabihf
OUT="target/${TARGET}/release"

LLVM_SIZE=$(find "$(rustc --print sysroot)" -name llvm-size -type f | head -1)
if [[ -z "${LLVM_SIZE}" ]]; then
    echo "llvm-size not found. Run: rustup component add llvm-tools-preview" >&2
    exit 1
fi

# `--locked` because the point of this harness is a number that does not move
# for reasons unrelated to the code being measured.
cargo build --release --locked --quiet

# `llvm-size -A` prints one "section size addr" row per section.
section() {  # section <binary> <section-name>
    "${LLVM_SIZE}" -A "$1" | awk -v s="$2" '$1 == s { print $2; found=1 } END { if (!found) print 0 }'
}

base_flash=$(( $(section "${OUT}/baseline" .text) + $(section "${OUT}/baseline" .rodata) ))
prot_flash=$(( $(section "${OUT}/protocol" .text) + $(section "${OUT}/protocol" .rodata) ))
base_bss=$(section "${OUT}/baseline" .bss)
prot_bss=$(section "${OUT}/protocol" .bss)

flash_delta=$(( prot_flash - base_flash ))
bss_delta=$(( prot_bss - base_bss ))

# `bc` is not guaranteed on a CI runner; awk is.
awk -v bf="${base_flash}" -v pf="${prot_flash}" -v fd="${flash_delta}" \
    -v bd="${bss_delta}" -v tgt="${TARGET}" 'BEGIN {
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
