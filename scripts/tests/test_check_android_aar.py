import io
import re
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))
sys.dont_write_bytecode = True

import check_android_aar
from check_android_aar import check, check_sources

CHECKER = REPO / "scripts" / "check_android_aar.py"

MANIFEST = """<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="com.offlineprotocol">
    <uses-permission android:name="android.permission.BLUETOOTH_CONNECT"/>
    <uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES"
        android:usesPermissionFlags="neverForLocation"/>
    <application>
        <service android:name="com.offlineprotocol.MeshForegroundService"/>
    </application>
</manifest>"""

RULES = """# What an application's R8 needs.
-keep class com.offlineprotocol.** { *; }
-keep class uniffi.** { *; }
-keep class com.sun.jna.** { *; }
-dontwarn com.google.errorprone.annotations.**
"""


def archive(entries, directories=()):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as out:
        for name in directories:
            out.writestr(name, b"")
        for name, content in entries.items():
            out.writestr(name, content)
    return buffer.getvalue()


def aar(classes=None, manifest=MANIFEST, rules=RULES, natives=None, drop=(), extra=None,
        classes_jar=None, directories=()):
    """A valid AAR, with whatever the test changes about it."""
    if classes is None:
        classes = list(check_android_aar.REQUIRED_CLASSES)
    entries = {
        "classes.jar": classes_jar or archive({name: b"" for name in classes}),
        "AndroidManifest.xml": manifest,
        "proguard.txt": rules,
        "R.txt": "",
    }
    for name in check_android_aar.LEGAL_FILES:
        entries[name] = f"the {name} text"
    for abi, libraries in (natives or {}).items():
        for library in libraries:
            entries[f"jni/{abi}/{library}"] = b"elf"
    entries.update(extra or {})
    for name in drop:
        del entries[name]
    return archive(entries, directories)


ALL_NATIVES = {abi: ["libuniffi_offline_protocol.so"] for abi in check_android_aar.ABIS}
WITH = list(check_android_aar.REQUIRED_CLASSES)


class CheckTests(unittest.TestCase):
    def assertProblem(self, problems, text):
        self.assertTrue(
            any(text in problem for problem in problems),
            f"no problem mentions {text!r}: {problems}",
        )

    def test_a_library_with_no_natives_passes_a_pull_request(self):
        self.assertEqual(check(aar(), require_natives=False), [])

    def test_a_library_with_every_abi_passes_a_release(self):
        self.assertEqual(check(aar(natives=ALL_NATIVES), require_natives=True), [])

    # What a real AAR looks like: every directory is an entry of its own.
    def test_directory_entries_are_not_files(self):
        directories = ["jni/"] + [f"jni/{abi}/" for abi in check_android_aar.ABIS]
        built = aar(natives=ALL_NATIVES, directories=directories)
        self.assertEqual(check(built, require_natives=True), [])

    # The failure the whole check exists for, once for each file left out.
    def test_each_react_only_class_is_refused(self):
        for source in check_android_aar.REACT_ONLY_SOURCES:
            name = "com/offlineprotocol/" + source.replace(".kt", ".class")
            problems = check(aar(classes=WITH + [name]), require_natives=False)
            self.assertProblem(problems, f"{name}, which needs React")

    # Kotlin compiles a lambda or a companion into a class of its own. The
    # module leaves dozens, and removing the one named after it removes none
    # of them.
    def test_a_nested_class_of_the_react_module_is_refused(self):
        nested = [
            "com/offlineprotocol/OfflineProtocolModule$Companion.class",
            "com/offlineprotocol/MeshHeadlessWakeService$getTaskConfig$1.class",
        ]
        problems = check(aar(classes=WITH + nested), require_natives=False)
        for name in nested:
            self.assertProblem(problems, name)

    def test_each_legal_file_is_required(self):
        for name in check_android_aar.LEGAL_FILES:
            problems = check(aar(drop=(name,)), require_natives=False)
            self.assertProblem(problems, f"it holds no {name} at its root")

    def test_an_empty_legal_file_is_refused(self):
        problems = check(aar(extra={"LICENSE": "  \n"}), require_natives=False)
        self.assertProblem(problems, "its LICENSE is empty")

    # A notice one directory down is not the one a reader of the AAR finds.
    def test_a_legal_file_below_the_root_is_not_at_the_root(self):
        built = aar(drop=("EXPORT.md",), extra={"legal/EXPORT.md": "text"})
        self.assertProblem(check(built, require_natives=False), "it holds no EXPORT.md at its root")

    # A class that only shares a prefix with one of them is the library's own.
    def test_a_class_that_shares_a_prefix_is_not_refused(self):
        classes = WITH + ["com/offlineprotocol/OfflineProtocolModuleHelper.class"]
        self.assertEqual(check(aar(classes=classes), require_natives=False), [])

    # Anything under com.facebook, not only what React Native's bridge holds.
    def test_a_class_of_react_itself_is_refused(self):
        for name in ("com/facebook/react/bridge/Promise.class", "com/facebook/soloader/SoLoader.class"):
            problems = check(aar(classes=WITH + [name]), require_natives=False)
            self.assertProblem(problems, f"{name}, a class of React itself")

    # An empty library holds nothing forbidden.
    def test_a_library_missing_a_part_is_refused(self):
        for missing in check_android_aar.REQUIRED_CLASSES:
            classes = [c for c in check_android_aar.REQUIRED_CLASSES if c != missing]
            problems = check(aar(classes=classes), require_natives=False)
            self.assertProblem(problems, f"does not hold {missing}")

    def test_a_classes_jar_that_is_not_an_archive_is_refused(self):
        problems = check(aar(classes_jar=b"not a zip"), require_natives=False)
        self.assertProblem(problems, "its classes.jar is not a zip archive")

    def test_a_second_jar_is_refused(self):
        problems = check(aar(extra={"libs/react.jar": b""}), require_natives=False)
        self.assertProblem(problems, "holds libs/react.jar, which this check does not look inside")

    def test_each_forbidden_manifest_entry_is_refused(self):
        for entry, name in (
            ('<service android:name="com.offlineprotocol.MeshHeadlessWakeService"/>',
             "com.offlineprotocol.MeshHeadlessWakeService"),
            ('<service android:name=".MeshHeadlessWakeService"/>',
             "com.offlineprotocol.MeshHeadlessWakeService"),
            ('<uses-permission android:name="android.permission.WAKE_LOCK"/>',
             "android.permission.WAKE_LOCK"),
        ):
            manifest = MANIFEST.replace("<application>", entry + "<application>")
            problems = check(aar(manifest=manifest), require_natives=False)
            self.assertProblem(problems, f"declares {name}")

    # A name in a comment declares nothing, either way round.
    def test_a_forbidden_entry_in_a_comment_is_not_an_entry(self):
        manifest = MANIFEST.replace(
            "<application>",
            '<!-- <uses-permission android:name="android.permission.WAKE_LOCK"/> --><application>',
        )
        self.assertEqual(check(aar(manifest=manifest), require_natives=False), [])

    def test_a_required_entry_in_a_comment_is_missing(self):
        for name in ("com.offlineprotocol.MeshForegroundService",
                     "android.permission.BLUETOOTH_CONNECT",
                     "android.permission.NEARBY_WIFI_DEVICES"):
            manifest = re.sub(rf'(<[^<]*"{re.escape(name)}"[^>]*>)', r"<!-- \1 -->", MANIFEST)
            self.assertIn("<!--", manifest)
            problems = check(aar(manifest=manifest), require_natives=False)
            self.assertProblem(problems, f"does not declare {name}")

    def test_a_required_permission_without_its_flag_is_refused(self):
        for name, flag in check_android_aar.REQUIRED_PERMISSION_FLAGS:
            manifest = MANIFEST.replace(f'android:usesPermissionFlags="{flag}"', "")
            self.assertNotIn(flag, manifest)
            problems = check(aar(manifest=manifest), require_natives=False)
            self.assertProblem(problems, f"declares {name} without usesPermissionFlags {flag}")

    def test_a_required_flag_beside_others_passes(self):
        manifest = MANIFEST.replace(
            'android:usesPermissionFlags="neverForLocation"',
            'android:usesPermissionFlags="neverForLocation|0x2"',
        )
        self.assertEqual(check(aar(manifest=manifest), require_natives=False), [])

    def test_a_required_flag_in_a_comment_is_missing(self):
        manifest = MANIFEST.replace(
            '<uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES"\n'
            '        android:usesPermissionFlags="neverForLocation"/>',
            '<uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES"/>'
            '<!-- android:usesPermissionFlags="neverForLocation" -->',
        )
        self.assertIn("<!--", manifest)
        problems = check(aar(manifest=manifest), require_natives=False)
        self.assertProblem(problems, "without usesPermissionFlags neverForLocation")

    def test_each_missing_rule_is_refused(self):
        for rule in check_android_aar.REQUIRED_RULES:
            rules = "\n".join(line for line in RULES.splitlines() if not line.startswith(rule))
            problems = check(aar(rules=rules), require_natives=False)
            self.assertProblem(problems, f"lacks the rule '{rule}'")

    def test_a_rule_in_a_comment_is_missing(self):
        for rule in check_android_aar.REQUIRED_RULES:
            rules = RULES.replace(rule, "# " + rule)
            problems = check(aar(rules=rules), require_natives=False)
            self.assertProblem(problems, f"lacks the rule '{rule}'")

    def test_each_missing_entry_is_refused(self):
        for name, text in (
            ("classes.jar", "no classes.jar"),
            ("AndroidManifest.xml", "no AndroidManifest.xml"),
            ("proguard.txt", "no proguard.txt"),
        ):
            problems = check(aar(drop=(name,)), require_natives=False)
            self.assertProblem(problems, text)

    def test_a_release_missing_an_abi_is_refused(self):
        for missing in check_android_aar.ABIS:
            natives = {abi: libs for abi, libs in ALL_NATIVES.items() if abi != missing}
            problems = check(aar(natives=natives), require_natives=True)
            self.assertProblem(problems, f"no native library for {missing}")

    # The same AAR on a pull request: what it has is fine, and it is not
    # asked for the rest.
    def test_a_pull_request_is_not_asked_for_every_abi(self):
        natives = {"arm64-v8a": ["libuniffi_offline_protocol.so"]}
        self.assertEqual(check(aar(natives=natives), require_natives=False), [])

    # A developer's tree holds builds under the name the library had before.
    def test_a_stale_native_library_is_refused(self):
        natives = dict(ALL_NATIVES)
        natives["arm64-v8a"] = ["libuniffi_offline_protocol.so", "liboffline_protocol_uniffi.so"]
        for require in (False, True):
            problems = check(aar(natives=natives), require_natives=require)
            self.assertProblem(problems, "holds liboffline_protocol_uniffi.so, which nothing loads")

    def test_an_abi_holding_only_the_wrong_library_is_refused(self):
        natives = {"arm64-v8a": ["liboffline_protocol_uniffi.so"]}
        problems = check(aar(natives=natives), require_natives=False)
        self.assertProblem(problems, "jni/arm64-v8a does not hold libuniffi_offline_protocol.so")

    def test_an_unknown_abi_is_refused(self):
        natives = {"mips": ["libuniffi_offline_protocol.so"]}
        problems = check(aar(natives=natives), require_natives=False)
        self.assertProblem(problems, "for mips, which is not an ABI")

    def test_a_library_at_the_wrong_depth_is_refused(self):
        for name in ("jni/arm64-v8a/sub/libuniffi_offline_protocol.so", "jni/stray.so"):
            problems = check(aar(extra={name: b"elf"}), require_natives=False)
            self.assertProblem(problems, f"{name}, which is not at jni/<abi>/<library>")

    def test_a_file_that_is_not_an_archive_is_refused(self):
        self.assertEqual(check(b"not a zip", require_natives=False), ["it is not a zip archive"])

    def test_every_problem_is_reported_not_the_first(self):
        classes = ["com/offlineprotocol/OfflineProtocolModule.class"]
        problems = check(aar(classes=classes, rules=""), require_natives=True)
        expected = 1 + len(check_android_aar.REQUIRED_CLASSES) + \
            len(check_android_aar.REQUIRED_RULES) + len(check_android_aar.ABIS)
        self.assertEqual(len(problems), expected, problems)


class SourcesTests(unittest.TestCase):
    def test_the_library_s_sources_pass(self):
        jar = archive({"com/offlineprotocol/InternetManager.kt": ""})
        self.assertEqual(check_sources(jar), [])

    def test_each_react_only_source_is_refused(self):
        for source in check_android_aar.REACT_ONLY_SOURCES:
            jar = archive({"com/offlineprotocol/InternetManager.kt": "",
                           "com/offlineprotocol/" + source: ""})
            self.assertEqual(
                check_sources(jar),
                [f"the sources jar holds com/offlineprotocol/{source}, which is not in the library"],
            )

    def test_an_empty_jar_is_refused(self):
        self.assertEqual(check_sources(archive({})), ["the sources jar holds no Kotlin source"])

    def test_a_file_that_is_not_an_archive_is_refused(self):
        self.assertEqual(check_sources(b"no"), ["the sources jar is not a zip archive"])


class CommandTests(unittest.TestCase):
    """The exit status is all a workflow reads."""

    def run_on(self, aar_bytes, *arguments, sources=None):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "library.aar"
            path.write_bytes(aar_bytes)
            command = [sys.executable, str(CHECKER), str(path), *arguments]
            if sources is not None:
                jar = Path(directory) / "sources.jar"
                jar.write_bytes(sources)
                command += ["--sources-jar", str(jar)]
            return subprocess.run(command, capture_output=True, text=True)

    def test_a_good_library_exits_zero(self):
        result = self.run_on(aar())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("holds the library", result.stdout)

    def test_a_bad_library_exits_non_zero_and_says_why(self):
        result = self.run_on(aar(rules=""))
        self.assertEqual(result.returncode, 1)
        self.assertIn("lacks the rule", result.stderr)

    def test_the_flag_reaches_the_check(self):
        self.assertEqual(self.run_on(aar()).returncode, 0)
        result = self.run_on(aar(), "--require-natives")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no native library for arm64-v8a", result.stderr)

    def test_the_sources_jar_is_checked_when_given(self):
        bad = archive({"com/offlineprotocol/OfflineProtocolModule.kt": ""})
        result = self.run_on(aar(), sources=bad)
        self.assertEqual(result.returncode, 1)
        self.assertIn("the sources jar holds", result.stderr)

    def test_a_missing_file_exits_non_zero(self):
        result = subprocess.run(
            [sys.executable, str(CHECKER), "/nonexistent/library.aar"],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("cannot read", result.stderr)


class MirrorTests(unittest.TestCase):
    """The lists here that also exist in a build file."""

    def test_the_react_only_list_is_the_build_file_s(self):
        build = (REPO / "bindings/kotlin/offline-protocol-android/build.gradle.kts").read_text()
        block = re.search(r"val reactOnly = listOf\((.*?)\)", build, flags=re.S)
        self.assertIsNotNone(block, "the build file no longer declares reactOnly as a listOf")
        self.assertEqual(
            sorted(re.findall(r'"([^"]+)"', block.group(1))),
            sorted(check_android_aar.REACT_ONLY_SOURCES),
        )

    def test_the_manifest_entries_are_the_build_file_s(self):
        build = (REPO / "bindings/kotlin/offline-protocol-android/build.gradle.kts").read_text()
        removed = []
        for name in ("reactOnlyServices", "reactOnlyPermissions"):
            block = re.search(rf"val {name} = listOf\((.*?)\)", build, flags=re.S)
            self.assertIsNotNone(block, f"the build file no longer declares {name}")
            removed += re.findall(r'"([^"]+)"', block.group(1))
        self.assertEqual(
            sorted(removed), sorted(name for _, name in check_android_aar.FORBIDDEN_IN_MANIFEST)
        )

    def test_the_legal_files_are_the_swift_package_s(self):
        script = (REPO / "scripts/assemble-swift-package.sh").read_text()
        block = re.search(r"^LEGAL_FILES=\((.*?)\)$", script, flags=re.M)
        self.assertIsNotNone(block, "the assemble script no longer declares LEGAL_FILES")
        self.assertEqual(sorted(block.group(1).split()), sorted(check_android_aar.LEGAL_FILES))

    def test_the_legal_files_are_the_build_file_s(self):
        build = (REPO / "bindings/kotlin/offline-protocol-android/build.gradle.kts").read_text()
        block = re.search(r"val legalFiles = listOf\((.*?)\)", build, flags=re.S)
        self.assertIsNotNone(block, "the build file no longer declares legalFiles as a listOf")
        self.assertEqual(
            sorted(re.findall(r'"([^"]+)"', block.group(1))), sorted(check_android_aar.LEGAL_FILES)
        )

    def test_the_abis_are_the_module_s(self):
        build = (REPO / "bindings/react-native/android/build.gradle").read_text()
        filters = re.search(r"abiFilters\s+(.*)", build)
        self.assertIsNotNone(filters, "the module no longer declares abiFilters")
        self.assertEqual(
            sorted(re.findall(r"'([^']+)'", filters.group(1))), sorted(check_android_aar.ABIS)
        )

    # Written out. A test that walks the list agrees with a list that lost
    # an entry.
    def test_the_required_rules_are_these(self):
        self.assertEqual(
            sorted(check_android_aar.REQUIRED_RULES),
            [
                "-dontwarn com.google.errorprone.annotations.**",
                "-keep class com.offlineprotocol.**",
                "-keep class com.sun.jna.**",
                "-keep class uniffi.**",
            ],
        )

    def test_every_required_rule_is_in_the_rules_the_library_ships(self):
        rules = check_android_aar.rules_in(
            (REPO / "bindings/react-native/android/consumer-rules.pro").read_text()
        )
        for rule in check_android_aar.REQUIRED_RULES:
            self.assertTrue(any(line.startswith(rule) for line in rules), rule)


if __name__ == "__main__":
    unittest.main()
