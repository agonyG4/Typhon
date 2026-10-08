import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
ARCHIVE_SCRIPT = REPO_ROOT / "bin" / "create-source-archive"


class SourceArchiveTests(unittest.TestCase):
    def test_archive_includes_observer_launchers_and_tooling_tests(self):
        with tempfile.TemporaryDirectory(prefix="typhon-source-archive-") as directory:
            archive_path = Path(directory) / "source.zip"
            result = subprocess.run(
                [str(ARCHIVE_SCRIPT), str(archive_path)],
                cwd=REPO_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=20,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(Path(result.stdout.strip()), archive_path)
            with zipfile.ZipFile(archive_path) as archive:
                entries = set(archive.namelist())

            self.assertTrue(
                {
                    "Typhon/bin/observe-compositor-exit.py",
                    "Typhon/bin/start-oblivion-one",
                    "Typhon/bin/start-oblivion-one-tty",
                    "Typhon/bin/start-session",
                    "Typhon/bin/install-start-oblivion-one",
                    "Typhon/bin/create-source-archive",
                    "Typhon/tests/tooling/test_process_exit_observer.py",
                    "Typhon/tests/tooling/test_source_archive.py",
                }.issubset(entries)
            )


if __name__ == "__main__":
    unittest.main()
