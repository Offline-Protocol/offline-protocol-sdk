"""Check that the Android library's unit suite ran, and passed.

Gradle succeeds on a test task that found no test, and the library reads its
sources from another directory, so a suite that compiled nothing is one wrong
path away. This reads the JUnit results Gradle wrote and refuses a run that is
empty, short, missing a named suite, or failed.

Run by the Android Library job on every pull request and by the release on the
library it publishes.

Usage:
    python3 scripts/check_android_test_results.py <test-results directory>
"""

import glob
import os
import re
import sys

# A suite that has to be among the results, and a floor on the count. The
# suite had 558 tests when this was written.
NAMED = "TEST-com.offlineprotocol.RelayControlOpTranslatorTest.xml"
MINIMUM = 400


def main(directory):
    results = glob.glob(os.path.join(directory, "*.xml"))
    tests = failures = 0
    for path in results:
        with open(path, encoding="utf-8") as handle:
            head = handle.read(4000)
        found = re.search(r'tests="(\d+)" skipped="\d+" failures="(\d+)" errors="(\d+)"', head)
        if not found:
            return f"cannot read the totals of {path}"
        tests += int(found.group(1))
        failures += int(found.group(2)) + int(found.group(3))
    if not any(os.path.basename(path) == NAMED for path in results):
        return f"{NAMED} is not among the results"
    if tests < MINIMUM:
        return f"only {tests} tests ran"
    if failures:
        return f"{failures} failed"
    print(f"{tests} tests in {len(results)} suites")
    return None


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    problem = main(sys.argv[1])
    if problem:
        sys.exit(f"ERROR: {problem}")
