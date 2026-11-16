import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))
sys.dont_write_bytecode = True

from check_manual_android_jni_libs import LOADED, STALE, check, main

SCRIPT = REPO / "scripts" / "check_manual_android_jni_libs.py"


class CheckManualJniLibsTests(unittest.TestCase):
    def test_missing_directory_is_ok(self):
        self.assertEqual(check(REPO / "no-such-jniLibs-dir"), [])

    def test_empty_tree_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.assertEqual(check(root), [])

    def test_loaded_name_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            abi = root / "arm64-v8a"
            abi.mkdir()
            (abi / LOADED).write_bytes(b"elf")
            self.assertEqual(check(root), [])
            completed = subprocess.run(
                [sys.executable, str(SCRIPT), str(root)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_stale_cargo_name_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            abi = root / "x86_64"
            abi.mkdir()
            (abi / STALE).write_bytes(b"elf")
            problems = check(root)
            self.assertEqual(len(problems), 1)
            self.assertIn(STALE, problems[0])
            completed = subprocess.run(
                [sys.executable, str(SCRIPT), str(root)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 1, completed.stdout)

    def test_unexpected_name_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "libwrong.so").write_bytes(b"elf")
            problems = check(root)
            self.assertEqual(len(problems), 1)
            self.assertIn("unexpected", problems[0])

    def test_cli_usage_errors_without_path(self):
        self.assertEqual(main(["check_manual_android_jni_libs.py"]), 2)