"""horizon-accel's Java side (mods/horizon-accel): compiles with javac 25 and runs its tests."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT / "mods/horizon-accel/src"


def jdk():
    """javac and java from JAVA_HOME or the PATH, if they are Java 25 or newer."""
    home = os.environ.get("JAVA_HOME")
    bin_dir = Path(home) / "bin" if home else None
    javac = str(bin_dir / "javac") if bin_dir and (bin_dir / "javac").exists() else shutil.which("javac")
    java = str(bin_dir / "java") if bin_dir and (bin_dir / "java").exists() else shutil.which("java")
    if not javac or not java:
        return None
    version = subprocess.run([javac, "-version"], capture_output=True, text=True).stdout.split()
    if len(version) < 2 or int(version[1].split(".")[0]) < 25:
        return None
    return javac, java


@unittest.skipUnless(jdk(), "needs a Java 25 JDK")
class AccelJavaTests(unittest.TestCase):
    def test_compiles_and_passes(self):
        javac, java = jdk()
        with tempfile.TemporaryDirectory() as out:
            sources = sorted(str(p) for p in SOURCES.rglob("*.java"))
            subprocess.run([javac, "--release", "25", "-Xlint:all,-restricted", "-Werror", "-d", out, *sources],
                           check=True)
            result = subprocess.run([java, "-ea", "-cp", out, "dev.horizonlab.accel.AccelTest"],
                                    capture_output=True, text=True, check=True)
            self.assertIn("AccelTest: ok", result.stdout)


if __name__ == "__main__":
    unittest.main()
