// Copyright (c) 2025-2026 Offline Protocol, Inc.
// SPDX-License-Identifier: AGPL-3.0-only (or a commercial license; see LICENSE-COMMERCIAL.md)

package com.offlineprotocol.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Set;
import java.util.TreeSet;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;

/**
 * The committed C header is the contract this binding declares for JNA. A function
 * added to the header and not declared here is unreachable from Java and fails no
 * build; a function declared here and gone from the header fails at the first call.
 * This holds the two lists, and the enum values the Java enums mirror, to the header.
 */
class HeaderDriftTest {
    private static String header() throws IOException {
        Path path = Paths.get("..", "c", "include", "offline_protocol.h");
        return new String(Files.readAllBytes(path), StandardCharsets.UTF_8);
    }

    @Test
    void everyHeaderFunctionIsDeclaredAndNothingElseIs() throws IOException {
        Set<String> inHeader = new TreeSet<>();
        Matcher m = Pattern.compile("(offline_protocol_[a-z_]+)\\(").matcher(header());
        while (m.find()) {
            inHeader.add(m.group(1));
        }
        Set<String> declared = new TreeSet<>();
        for (Method method : NativeApi.class.getDeclaredMethods()) {
            if (method.getName().startsWith("offline_protocol_")) {
                declared.add(method.getName());
            }
        }
        assertTrue(inHeader.size() >= 12, "the header scan found " + inHeader);
        assertEquals(inHeader, declared);
    }

    @Test
    void theEnumsMirrorTheHeaderValues() throws IOException {
        String header = header();
        for (Status s : Status.values()) {
            assertTrue(!valueLine(header, "OfflineProtocolStatus_", s.value()).isEmpty(),
                    s + " is not " + s.value() + " in the header");
        }
        for (Transport t : Transport.values()) {
            assertTrue(!valueLine(header, "OfflineProtocolTransport_", t.value()).isEmpty(),
                    t + " is not " + t.value() + " in the header");
        }
        for (StorageException.Kind k : StorageException.Kind.values()) {
            assertTrue(!valueLine(header, "OfflineProtocolStorageStatus_", k.value()).isEmpty(),
                    k + " is not " + k.value() + " in the header");
        }
    }

    /** The header line {@code <prefix>Name = value,}, or empty if no member of that prefix has the value. */
    private static String valueLine(String header, String prefix, int value) {
        Matcher m = Pattern.compile(prefix + "[A-Za-z]+ = " + value + ",").matcher(header);
        return m.find() ? m.group() : "";
    }
}
