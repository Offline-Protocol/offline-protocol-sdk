# Compiles and runs main.c against the SDK's C API, linked once against the
# shared library and once against the static one, and compiles the header as
# C++. Build the library first:
#
#   cargo build -p offline-protocol-uniffi
#   make -C bindings/c/consumer-check
#
# PROFILE=release checks a release build. NATIVE_STATIC_LIBS is what the
# static library needs from the system; it is read from rustc unless given.

ROOT     := $(abspath ../../..)
PROFILE  ?= debug
LIBDIR   ?= $(ROOT)/target/$(PROFILE)
LIBNAME  := offline_protocol_uniffi
INCLUDE  := $(ROOT)/bindings/c/include
BUILD    := build

CC       ?= cc
CXX      ?= c++
CFLAGS   := -std=c99 -Wall -Wextra -Wpedantic -Werror -I$(INCLUDE)
CXXFLAGS := -std=c++11 -Wall -Wextra -Wpedantic -Werror -I$(INCLUDE)

ifeq ($(shell uname),Darwin)
SHARED_EXT := dylib
else
SHARED_EXT := so
endif

# rustc prints the system libraries a staticlib needs on its own line; the
# library itself does not record them anywhere a linker can find.
NATIVE_STATIC_LIBS ?= $(shell cd $(ROOT) && cargo rustc -q -p offline-protocol-uniffi --lib \
	$(if $(filter release,$(PROFILE)),--release,) -- --print native-static-libs 2>&1 \
	| sed -n 's/.*native-static-libs: //p' | tail -1)

.PHONY: all run clean
all: run

$(BUILD):
	mkdir -p $(BUILD)

$(BUILD)/check-shared: main.c $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CC) $(CFLAGS) main.c -L$(LIBDIR) -l$(LIBNAME) -Wl,-rpath,$(LIBDIR) -lpthread -o $@

# The archive is named by path: with both libraries in one directory a
# linker given -l picks the shared one.
$(BUILD)/check-static: main.c $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CC) $(CFLAGS) main.c $(LIBDIR)/lib$(LIBNAME).a $(NATIVE_STATIC_LIBS) -lpthread -o $@

$(BUILD)/header-cxx.o: header_check.cpp $(INCLUDE)/offline_protocol.h | $(BUILD)
	$(CXX) $(CXXFLAGS) -c header_check.cpp -o $@

run: $(BUILD)/check-shared $(BUILD)/check-static $(BUILD)/header-cxx.o
	./$(BUILD)/check-shared
	./$(BUILD)/check-static

clean:
	rm -rf $(BUILD)
