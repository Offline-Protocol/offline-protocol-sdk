#!/usr/bin/env python3
"""Check what is inside the Android library's AAR, and its sources jar.

The library is the React Native module's bridge, built without the files that
need React. Nothing in the build says whether that worked: an AAR that still
holds the React module compiles, publishes, and fails in the application that
adds it, with a missing class from a library it never asked for. A version on
a Maven repository cannot be replaced, so this looks first.

Usage:
    python3 scripts/check_android_aar.py <aar> [--sources-jar <jar>] [--require-natives]

--require-natives is for a release: every ABI must be present. Without it an
AAR with no native library passes, which is what a pull request builds, but
one that holds any is still held to the same rule for the ABIs it has.
"""

import argparse
import io
import re
import sys
import zipfile

# The sources that need React, by file name. The library's build file holds
# the same list and a test compares the two.
REACT_ONLY_SOURCES = (
    "OfflineProtocolModule.kt",
    "OfflineProtocolPackage.kt",
    "MeshHeadlessWakeService.kt",
)
BRIDGE_PACKAGE = "com/offlineprotocol/"
REACT_PACKAGE = "com/facebook/"

# One class from each part, so that an AAR holding only part of the library
# does not pass for holding none of what is forbidden.
REQUIRED_CLASSES = (
    "uniffi/offline_protocol/OfflineProtocol.class",
    "com/offlineprotocol/InternetManager.class",
    "com/offlineprotocol/ble/BleTransportFacade.class",
    "com/offlineprotocol/mesh/MeshController.class",
    "com/offlineprotocol/MlsSecureStorage.class",
    "com/offlineprotocol/MeshForegroundService.class",
)

# By the `android:name` an element declares, not by the text of the file: a
# name inside a comment declares nothing.
FORBIDDEN_IN_MANIFEST = (
    ("service", "com.offlineprotocol.MeshHeadlessWakeService"),
    ("uses-permission", "android.permission.WAKE_LOCK"),
)
REQUIRED_IN_MANIFEST = (
    ("service", "com.offlineprotocol.MeshForegroundService"),
    ("uses-permission", "android.permission.BLUETOOTH_CONNECT"),
)

# The rules an application's R8 needs, or its release build loses the FFI, or
# does not finish at all.
REQUIRED_RULES = (
    "-keep class uniffi.**",
    "-keep class com.sun.jna.**",
    "-keep class com.offlineprotocol.**",
    "-dontwarn com.google.errorprone.annotations.**",
)

# The ABIs the module builds. A test compares this with its build file.
ABIS = ("arm64-v8a", "armeabi-v7a", "x86", "x86_64")
NATIVE_LIBRARY = "libuniffi_offline_protocol.so"

ANDROID_NAME = re.compile(r"""android:name\s*=\s*["']([^"']+)["']""")


def react_only_classes():
    return tuple(BRIDGE_PACKAGE + name[: -len(".kt")] for name in REACT_ONLY_SOURCES)


def declared(manifest, tag, package):
    """The `android:name` of every <tag> element, with comments removed and a
    leading dot resolved against the manifest's package."""
    manifest = re.sub(r"<!--.*?-->", "", manifest, flags=re.S)
    names = set()
    for element in re.findall(rf"<{re.escape(tag)}\b[^>]*>", manifest):
        match = ANDROID_NAME.search(element)
        if match:
            name = match.group(1)
            names.add(package + name if name.startswith(".") else name)
    return names


def rules_in(text):
    """The rules of a ProGuard file: its lines, less comments and blanks."""
    lines = (line.split("#", 1)[0].strip() for line in text.splitlines())
    return [line for line in lines if line]


def members(archive):
    """The files of a zip, without its directory entries."""
    return [name for name in archive.namelist() if not name.endswith("/")]


def check_sources(jar_bytes):
    """What is wrong with the sources jar. Empty means nothing."""
    try:
        jar = zipfile.ZipFile(io.BytesIO(jar_bytes))
    except zipfile.BadZipFile:
        return ["the sources jar is not a zip archive"]

    problems = []
    names = members(jar)
    for name in sorted(names):
        if name.rsplit("/", 1)[-1] in REACT_ONLY_SOURCES:
            problems.append(f"the sources jar holds {name}, which is not in the library")
    if not any(name.endswith(".kt") for name in names):
        problems.append("the sources jar holds no Kotlin source")
    return problems


def check(aar_bytes, require_natives):
    """What is wrong with the AAR. Empty means nothing."""
    problems = []

    try:
        aar = zipfile.ZipFile(io.BytesIO(aar_bytes))
    except zipfile.BadZipFile:
        return ["it is not a zip archive"]

    names = members(aar)

    if "classes.jar" not in names:
        problems.append("it holds no classes.jar")
    else:
        try:
            classes = set(members(zipfile.ZipFile(io.BytesIO(aar.read("classes.jar")))))
        except zipfile.BadZipFile:
            classes = None
            problems.append("its classes.jar is not a zip archive")

        if classes is not None:
            forbidden = react_only_classes()
            for name in sorted(classes):
                # A nested or synthetic class carries its owner's name and a
                # suffix after `$`.
                stem = name[: -len(".class")] if name.endswith(".class") else name
                if stem.split("$", 1)[0] in forbidden:
                    problems.append(f"classes.jar holds {name}, which needs React")
                elif name.startswith(REACT_PACKAGE):
                    problems.append(f"classes.jar holds {name}, a class of React itself")

            for name in REQUIRED_CLASSES:
                if name not in classes:
                    problems.append(f"classes.jar does not hold {name}")

    # A jar beside classes.jar is code the checks above never opened.
    for name in sorted(names):
        if name.startswith("libs/") and name.endswith(".jar"):
            problems.append(f"it holds {name}, which this check does not look inside")

    if "AndroidManifest.xml" not in names:
        problems.append("it holds no AndroidManifest.xml")
    else:
        manifest = aar.read("AndroidManifest.xml").decode("utf-8", errors="replace")
        package = re.search(r"""<manifest\b[^>]*\bpackage\s*=\s*["']([^"']+)["']""", manifest)
        package = package.group(1) if package else ""
        for tag, name in FORBIDDEN_IN_MANIFEST:
            if name in declared(manifest, tag, package):
                problems.append(
                    f"the manifest declares {name}, which only React Native needs"
                )
        for tag, name in REQUIRED_IN_MANIFEST:
            if name not in declared(manifest, tag, package):
                problems.append(f"the manifest does not declare {name}")

    if "proguard.txt" not in names:
        problems.append("it holds no proguard.txt, so an application's R8 strips the FFI")
    else:
        rules = rules_in(aar.read("proguard.txt").decode("utf-8", errors="replace"))
        for rule in REQUIRED_RULES:
            if not any(line.startswith(rule) for line in rules):
                problems.append(f"proguard.txt lacks the rule '{rule}'")

    natives = {}
    for name in names:
        parts = name.split("/")
        if parts[0] != "jni":
            continue
        if len(parts) != 3:
            problems.append(f"it holds {name}, which is not at jni/<abi>/<library>")
            continue
        natives.setdefault(parts[1], []).append(parts[2])

    for abi, libraries in sorted(natives.items()):
        if abi not in ABIS:
            problems.append(f"it holds native libraries for {abi}, which is not an ABI it builds")
        for library in sorted(libraries):
            if library != NATIVE_LIBRARY:
                # A second library is a stale build under an old name. It is
                # packaged into every application, and nothing loads it.
                problems.append(f"jni/{abi} holds {library}, which nothing loads")
        if NATIVE_LIBRARY not in libraries:
            problems.append(f"jni/{abi} does not hold {NATIVE_LIBRARY}")

    if require_natives:
        for abi in ABIS:
            if abi not in natives:
                problems.append(f"it holds no native library for {abi}")

    return problems


def read(path):
    with open(path, "rb") as handle:
        return handle.read()


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("aar")
    parser.add_argument("--sources-jar")
    parser.add_argument("--require-natives", action="store_true")
    arguments = parser.parse_args(argv)

    try:
        problems = check(read(arguments.aar), arguments.require_natives)
        if arguments.sources_jar:
            problems += check_sources(read(arguments.sources_jar))
    except OSError as error:
        print(f"ERROR: cannot read {error.filename}: {error.strerror}", file=sys.stderr)
        return 1

    if problems:
        print(f"ERROR: {arguments.aar} is not the library:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    print(f"{arguments.aar}: holds the library and nothing that needs React")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
