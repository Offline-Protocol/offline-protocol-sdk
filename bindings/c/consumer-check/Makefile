# Compiles and runs the C programs against the SDK's C API:
#
# - main.c, the contract, linked once against the shared library and once
#   against the static one;
# - exchange.c, two engines with a C host carrying frames between them;
# - the quick start in bindings/c/README.md, extracted and run, so the
#   example a reader copies cannot rot;
#
# and compiles the header as C++. Build the library first:
#
#   cargo build -p offline-protocol-uniffi
#   make -C bindings/c/consumer-check
#
# PROFILE=release checks a release build. NATIVE_STATIC_LIBS is what the
# static library needs from the system; it is read from rustc unless given.
#
# SANITIZE=1 builds every program with AddressSanitizer and
# UndefinedBehaviorSanitizer (and LeakSanitizer, where the platform has it:
# Linux, not macOS), into a directory of its own. The library allocates
# with the system allocator, so a library string freed twice, freed with the
# host's own free, or never freed is reported at the line that did it: the
# ownership rule (K1) checked rather than read.

ROOT     := $(abspath ../../..)
PROFILE  ?= debug
LIBDIR   ?= $(ROOT)/target/$(PROFILE)
LIBNAME  := offline_protocol_uniffi
INCLUDE  := $(ROOT)/bindings/c/include
SANITIZE ?= 0
ifeq ($(SANITIZE),1)
SAN      := -g -fno-omit-frame-pointer -fsanitize=address,undefined -fno-sanitize-recover=undefined
BUILD    := build-sanitize
else
SAN      :=
BUILD    := build
endif

CC       ?= cc
CXX      ?= c++
CFLAGS   := -std=c99 -Wall -Wextra -Wpedantic -Werror -I$(INCLUDE) $(SAN)
CXXFLAGS := -std=c++11 -Wall -Wextra -Wpedantic -Werror -I$(INCLUDE) $(SAN)
SHARED   := -L$(LIBDIR) -l$(LIBNAME) -Wl,-rpath,$(LIBDIR) -lpthread

# rustc prints the system libraries a staticlib needs on its own line; the
# library itself does not record them anywhere a linker can find.
#
# Two traps in reading that line. Under CARGO_TERM_COLOR=always (CI sets it
# for the whole workflow) cargo passes --color always to rustc, and the line
# ends in an escape sequence that the linker then reads as part of the last
# library's name (`-lc<ESC>[0m`): hence --color never, and the escapes
# stripped besides for a cargo that colours anyway. And `cargo rustc` with
# extra arguments is fingerprinted apart from `cargo build`, so this probe
# relinks the library; CI builds through the probe once and passes the flags
# in, and a local run after `cargo build` pays one extra link.
ESC := $(shell printf '\033')
NATIVE_STATIC_LIBS ?= $(shell cd $(ROOT) && cargo rustc -q --color never -p offline-protocol-uniffi --lib \
	$(if $(filter release,$(PROFILE)),--release,) -- --print native-static-libs 2>&1 \
	| sed -n 's/.*native-static-libs: //p' | tail -1 | sed 's/$(ESC)\[[0-9;]*m//g')

.PHONY: all run clean
all: run

$(BUILD):
	mkdir -p $(BUILD)

$(BUILD)/check-shared: main.c $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CC) $(CFLAGS) main.c $(SHARED) -o $@

$(BUILD)/exchange: exchange.c $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CC) $(CFLAGS) exchange.c $(SHARED) -o $@

# The quick start in the README, compiled as written; the run below fills
# in its placeholders first (quickstart.sh says which).
README   := $(ROOT)/bindings/c/README.md

$(BUILD)/quickstart.c: $(README) quickstart.sh | $(BUILD)
	./quickstart.sh $(README) $@

$(BUILD)/quickstart: $(BUILD)/quickstart.c $(INCLUDE)/offline_protocol.h
	$(CC) $(CFLAGS) $(BUILD)/quickstart.c $(SHARED) -o $@

# The archive is named by path: with both libraries in one directory a
# linker given -l picks the shared one.
$(BUILD)/check-static: main.c $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CC) $(CFLAGS) main.c $(LIBDIR)/lib$(LIBNAME).a $(NATIVE_STATIC_LIBS) -lpthread -o $@

$(BUILD)/header-cxx.o: header_check.cpp $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CXX) $(CXXFLAGS) -c header_check.cpp -o $@

run: $(BUILD)/check-shared $(BUILD)/check-static $(BUILD)/exchange $(BUILD)/quickstart $(BUILD)/header-cxx.o
	./$(BUILD)/check-shared
	./$(BUILD)/check-static
	./$(BUILD)/exchange
	set -e; dir="$$(mktemp -d)"; trap 'rm -rf "$$dir"' EXIT; \
	  ./quickstart.sh $(README) "$$dir/quickstart.c" "$$dir"; \
	  $(CC) $(CFLAGS) "$$dir/quickstart.c" $(SHARED) -o "$$dir/quickstart"; \
	  out="$$("$$dir/quickstart")"; \
	  case "$$out" in *'this device is "off1'*) ;; *) echo "quick start printed: $$out"; exit 1;; esac; \
	  test -d "$$dir/mls" && test -d "$$dir/state"; \
	  echo "c quick start: ok"

clean:
	rm -rf build build-sanitize
