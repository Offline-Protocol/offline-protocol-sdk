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


def tree(root, files):
    for name in files:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"elf")


def run(root):
    return subprocess.run(
        [sys.executable, "-B", str(SCRIPT), str(root)],
        capture_output=True,
        text=True,
        check=False,
    )


class CheckManualJniLibsTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def assertProblem(self, problems, fragment):
        self.assertTrue(
            any(fragment in problem for problem in problems),
            f"no problem mentions {fragment!r}: {problems}",
        )

    def test_loaded_name_in_every_abi_passes(self):
        tree(self.root, [f"arm64-v8a/{LOADED}", f"x86_64/{LOADED}"])
        self.assertEqual(check(self.root), [])
        completed = run(self.root)
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_other_native_libraries_are_the_applications_own(self):
        tree(
            self.root,
            [f"arm64-v8a/{LOADED}", "arm64-v8a/libc++_shared.so", "arm64-v8a/libjnidispatch.so"],
        )
        self.assertEqual(check(self.root), [])

    def test_stale_cargo_name_fails(self):
        tree(self.root, [f"x86_64/{STALE}"])
        problems = check(self.root)
        self.assertProblem(problems, f"holds {STALE}, which nothing loads")
        self.assertProblem(problems, f"not {LOADED}")
        self.assertEqual(run(self.root).returncode, 1)

    def test_stale_name_beside_the_loaded_one_still_fails(self):
        tree(self.root, [f"arm64-v8a/{LOADED}", f"arm64-v8a/{STALE}"])
        problems = check(self.root)
        self.assertEqual(len(problems), 1, problems)
        self.assertProblem(problems, f"holds {STALE}")

    def test_an_abi_without_the_loaded_name_fails(self):
        tree(self.root, [f"arm64-v8a/{LOADED}", "armeabi-v7a/libc++_shared.so"])
        problems = check(self.root)
        self.assertEqual(len(problems), 1, problems)
        self.assertProblem(problems, f"armeabi-v7a: holds native libraries but not {LOADED}")

    def test_a_library_outside_an_abi_directory_fails(self):
        tree(self.root, [f"arm64-v8a/{LOADED}", LOADED])
        self.assertProblem(check(self.root), "outside an ABI directory")

    def test_a_tree_that_ships_nothing_fails(self):
        (self.root / "arm64-v8a").mkdir()
        self.assertProblem(check(self.root), f"no ABI directory holds {LOADED}")
        self.assertEqual(run(self.root).returncode, 1)

    def test_a_path_that_is_not_a_directory_is_an_error_not_a_pass(self):
        completed = run(self.root / "no-such-jniLibs")
        self.assertEqual(completed.returncode, 2)
        self.assertIn("is not a directory", completed.stderr)

    def test_usage_without_a_path(self):
        self.assertEqual(main(["check_manual_android_jni_libs.py"]), 2)


if __name__ == "__main__":
    unittest.main()
