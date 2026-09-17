"""Run on Linux: python3 scripts/rendezvous/test-update-relay.py."""
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import hashlib

spec = importlib.util.spec_from_file_location("updater", Path(__file__).with_name("update-relay.py"))
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


class UpdaterTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.binary = self.directory / "installed"
        self.binary.write_bytes(b"old")
        self.staged = self.directory / "staged"
        self.staged.write_bytes(b"new")

    def test_success_retains_backup(self):
        with patch.object(updater, "peer_id", return_value="same-peer"), \
             patch.object(updater, "run", return_value="active"), \
             patch.object(updater, "healthy", return_value=True) as health:
            updater.install(self.staged, self.binary, "relay", 36521)
        self.assertEqual(self.binary.read_bytes(), b"new")
        self.assertEqual(self.binary.with_name("installed.previous").read_bytes(), b"old")
        health.assert_called_once_with("relay", 36521, "same-peer")

    def test_failed_health_restores_old_binary(self):
        with patch.object(updater, "peer_id", return_value="original"), \
             patch.object(updater, "run", return_value="active"), \
             patch.object(updater, "healthy", side_effect=[False, True]), \
             self.assertRaisesRegex(RuntimeError, "health or peer-identity"):
            updater.install(self.staged, self.binary, "relay", 36521)
        self.assertEqual(self.binary.read_bytes(), b"old")

    def test_restart_failure_restores_old_binary(self):
        with patch.object(updater, "peer_id", return_value="original"), \
             patch.object(updater, "run", side_effect=["active", RuntimeError("restart failed"), ""]), \
             patch.object(updater, "healthy", return_value=True), \
             self.assertRaisesRegex(RuntimeError, "restart failed"):
            updater.install(self.staged, self.binary, "relay", 36521)
        self.assertEqual(self.binary.read_bytes(), b"old")

    def test_unhealthy_existing_service_is_untouched(self):
        with patch.object(updater, "peer_id", return_value="original"), \
             patch.object(updater, "run", return_value="inactive"), \
             self.assertRaisesRegex(RuntimeError, "unhealthy"):
            updater.install(self.staged, self.binary, "relay", 36521)
        self.assertEqual(self.binary.read_bytes(), b"old")

    def test_version_accepts_the_build_stamp_and_still_rejects_prereleases(self):
        # `enox --version` names the channel and commit it was built from, and
        # both call sites feed that whole line to version().
        self.assertEqual(updater.version("enox 0.9.0 (release, 1a2b3c4d5e6f)"), (0, 9, 0))
        self.assertEqual(updater.version("enox 0.9.0 (dev, 1a2b3c4d5e6f-dirty)"), (0, 9, 0))
        self.assertEqual(updater.version("enox 0.9.0 (dev, unknown)"), (0, 9, 0))
        # The forms that predate the stamp still parse.
        self.assertEqual(updater.version("enox 0.9.0"), (0, 9, 0))
        self.assertEqual(updater.version("v0.9.0"), (0, 9, 0))
        self.assertEqual(updater.version("0.9.0"), (0, 9, 0))
        # The relay installs stable releases only.
        for rejected in ("enox 0.9.0-rc1", "enox 0.9.0 (dev", "enox 0.9 (dev, abc)"):
            with self.assertRaisesRegex(ValueError, "stable semantic version"):
                updater.version(rejected)

    def test_same_version_and_downgrades_do_not_restart(self):
        for tag in ("v0.6.1", "v0.5.0"):
            with patch.object(updater, "fetch", return_value=io.BytesIO(json.dumps({"tag_name": tag}).encode())), \
                 patch.object(updater, "run", return_value="enox 0.6.1"), \
                 patch.object(updater, "install") as install:
                updater.update(SimpleNamespace(binary=self.binary, check=False))
                install.assert_not_called()

    def test_prerelease_is_rejected(self):
        with patch.object(updater, "fetch", return_value=io.BytesIO(b'{"tag_name":"v0.7.0","prerelease":true}')), \
             self.assertRaisesRegex(ValueError, "prerelease"):
            updater.update(SimpleNamespace(binary=self.binary, check=False))

    def archive(self, symlink=False):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            member = tarfile.TarInfo("enox")
            if symlink:
                member.type = tarfile.SYMTYPE
                member.linkname = "/etc/passwd"
                archive.addfile(member)
            else:
                member.size = 3
                archive.addfile(member, io.BytesIO(b"new"))
        return buffer.getvalue()

    def download(self, content, checksum=None):
        checksum = checksum or hashlib.sha256(content).hexdigest()
        return patch.object(updater, "fetch", side_effect=[
            io.BytesIO(f"{checksum}  enoxian-linux-x86_64.tar.gz\n".encode()), io.BytesIO(content)])

    def test_verified_archive_is_staged(self):
        with self.download(self.archive()), patch.object(updater, "run", return_value="enox 0.6.2"):
            staged = updater.stage_release("v0.6.2", "x86_64", self.directory)
        self.assertEqual(staged.read_bytes(), b"new")
        self.assertEqual(self.binary.read_bytes(), b"old")

    def test_bad_checksum_never_executes_download(self):
        with self.download(self.archive(), "0" * 64), patch.object(updater, "run") as run, \
             self.assertRaisesRegex(ValueError, "checksum mismatch"):
            updater.stage_release("v0.6.2", "x86_64", self.directory)
        run.assert_not_called()

    def test_symlink_archive_is_rejected(self):
        with self.download(self.archive(symlink=True)), self.assertRaisesRegex(ValueError, "regular"):
            updater.stage_release("v0.6.2", "x86_64", self.directory)

    def test_mismatched_binary_version_is_rejected(self):
        with self.download(self.archive()), patch.object(updater, "run", return_value="enox 0.5.0"), \
             self.assertRaisesRegex(ValueError, "does not match"):
            updater.stage_release("v0.6.2", "x86_64", self.directory)

    def test_peer_identity_change_fails_health(self):
        with patch.object(updater, "run", return_value="active"), \
             patch.object(updater, "peer_id", return_value="wrong-peer"), \
             patch.object(updater.time, "monotonic", side_effect=[0, 0, 31]), \
             patch.object(updater.time, "sleep"):
            self.assertFalse(updater.healthy("relay", 36521, "original"))


if __name__ == "__main__":
    unittest.main()
