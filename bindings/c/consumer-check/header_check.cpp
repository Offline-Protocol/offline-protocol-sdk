// The header compiles as C++ and its declarations have C linkage there.
#include "offline_protocol.h"

static_assert(sizeof(OfflineProtocolStatus) == sizeof(int), "a C enum");

int main() {
    OfflineProtocolStatus (*create)(const char *, OfflineProtocolHandle **, OfflineProtocolError *) =
        offline_protocol_create;
    return create == nullptr ? 1 : 0;
}
