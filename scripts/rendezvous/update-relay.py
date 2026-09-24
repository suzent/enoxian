#!/usr/bin/env python3
"""Install a checksum-verified stable relay release; roll back failed restarts."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

RELEASE_API = "https://api.github.com/repos/suzent/enoxian/releases/latest"
RELEASE_BASE = "https://github.com/suzent/enoxian/releases/download"
# The updater ships with each release so an installed copy can follow changes
# it could not have anticipated — such as a new `enox --version` format, which
# left every pre-0.9.0 copy failing before it could install anything.
UPDATER_ASSET = "update-relay.py"
REFRESHED_ENV = "ENOXIAN_RELAY_UPDATER_REFRESHED"


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "enoxian-relay-updater"})
    return urllib.request.urlopen(request, timeout=30)


def version(text):
    """Parse a release tag or an `enox --version` line into a version tuple.

    `enox --version` carries a build stamp — `enox 0.9.0 (release, 1a2b3c4d)` —
    so the trailing parenthesis is accepted and ignored. Only the three numbers
    decide whether an update is an upgrade; a prerelease suffix is still
    rejected, since the relay installs stable releases only.
    """
    match = re.fullmatch(
        r"(?:enox )?v?(\d+)\.(\d+)\.(\d+)(?: \([^()]*\))?", text.strip()
    )
    if not match:
        raise ValueError("Expected a stable semantic version: " + text)
    return tuple(map(int, match.groups()))


def run(*args):
    return subprocess.check_output(args, text=True, timeout=30).strip()


def peer_id(port):
    with fetch(f"http://127.0.0.1:{port}/peer-id") as response:
        peer = json.load(response)["peer_id"]
    if not isinstance(peer, str) or not peer:
        raise ValueError("Relay returned no peer identity")
    return peer


def healthy(service, port, expected):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            if run("systemctl", "is-active", service) == "active" and peer_id(port) == expected:
                return True
        except Exception:
            pass
        time.sleep(1)
    return False


def checksums(tag):
    with fetch(f"{RELEASE_BASE}/{tag}/SHA256SUMS") as response:
        return response.read().decode("utf-8").splitlines()


def checksum_of(lines, asset):
    """The one well-formed checksum `lines` lists for `asset`, or None."""
    hashes = [line.split()[0] for line in lines
              if len(line.split()) == 2 and line.split()[1].lstrip("*") == asset]
    if len(hashes) != 1 or not re.fullmatch(r"[0-9a-fA-F]{64}", hashes[0]):
        return None
    return hashes[0].lower()


def refresh_self(tag, updater):
    """Install the updater shipped with `tag` over `updater`; True if it changed.

    Verified against the release's SHA256SUMS like the binary, and required to
    compile, so a bad download cannot replace a working updater. A release that
    predates shipping the updater leaves it alone.
    """
    expected = checksum_of(checksums(tag), UPDATER_ASSET)
    if expected is None:
        return False
    with fetch(f"{RELEASE_BASE}/{tag}/{UPDATER_ASSET}") as response:
        body = response.read(1024 * 1024 + 1)
    if len(body) > 1024 * 1024 or hashlib.sha256(body).hexdigest() != expected:
        raise ValueError("Updater checksum mismatch; installed updater unchanged")
    if body == updater.read_bytes():
        return False
    compile(body, str(updater), "exec")
    staged = updater.with_name(f".{updater.name}.new")
    staged.write_bytes(body)
    staged.chmod(0o755)
    os.replace(staged, updater)
    return True


def stage_release(tag, arch, directory):
    asset = f"enoxian-linux-{arch}.tar.gz"
    base = f"{RELEASE_BASE}/{tag}"
    expected = checksum_of(checksums(tag), asset)
    if expected is None:
        raise ValueError("Missing or ambiguous asset checksum")
    archive = directory / asset
    digest = hashlib.sha256()
    with fetch(base + "/" + asset) as response, archive.open("wb") as output:
        while True:
            chunk = response.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
            output.write(chunk)
    if digest.hexdigest() != expected:
        raise ValueError("Release checksum mismatch; installed binary unchanged")
    staged = directory / "enox"
    # Extract only the executable, never archive paths or links.
    with tarfile.open(archive, "r:gz") as package:
        members = [m for m in package.getmembers() if m.name in ("enox", "./enox")]
        if len(members) != 1 or not members[0].isfile():
            raise ValueError("Archive must contain one regular enox executable")
        with package.extractfile(members[0]) as source, staged.open("wb") as output:
            shutil.copyfileobj(source, output)
    staged.chmod(0o755)
    if version(run(str(staged), "--version")) != version(tag):
        raise ValueError("Release binary version does not match tag")
    return staged


def install(staged, binary, service, port):
    expected_peer = peer_id(port)
    if run("systemctl", "is-active", service) != "active":
        raise RuntimeError("Refusing to update an unhealthy relay")
    backup = binary.with_name(binary.name + ".previous")
    backup_stage = staged.parent / "previous"
    shutil.copy2(binary, backup_stage)
    os.replace(backup_stage, backup)
    try:
        os.replace(staged, binary)
        run("systemctl", "restart", service)
        if not healthy(service, port, expected_peer):
            raise RuntimeError("New relay failed health or peer-identity check")
    except BaseException:
        # Retain the backup after rollback too. Never replace the peer key.
        restore = staged.parent / "restore"
        shutil.copy2(backup, restore)
        os.replace(restore, binary)
        run("systemctl", "restart", service)
        if not healthy(service, port, expected_peer):
            raise RuntimeError("CRITICAL: rollback also failed relay health check")
        print("Rolled back to previous healthy relay", flush=True)
        raise


def update(args):
    with fetch(RELEASE_API) as response:
        release = json.load(response)
    if release.get("draft") or release.get("prerelease"):
        raise ValueError("Refusing an unpublished or prerelease build")
    tag = release["tag_name"]
    available = version(tag)
    # Before anything that could fail on an assumption this copy makes about
    # the release: a newer updater may be what knows how to read it.
    updater = getattr(args, "updater", None)
    if updater and not args.check and not os.environ.get(REFRESHED_ENV):
        if refresh_self(tag, updater):
            print(f"Updated the relay updater to the one shipped with {tag}", flush=True)
            os.environ[REFRESHED_ENV] = "1"
            # The lock is close-on-exec, so the new updater takes it afresh.
            os.execv(sys.executable, [sys.executable, str(updater), *sys.argv[1:]])
    current = version(run(str(args.binary), "--version"))
    if available <= current:
        print(f"Relay is current ({'.'.join(map(str, current))}); no restart needed")
        return
    arch = {"x86_64": "x86_64", "aarch64": "aarch64"}.get(os.uname().machine)
    if not arch:
        raise ValueError("Unsupported relay architecture")
    print(f"Stable relay update available: {tag}", flush=True)
    if args.check:
        return
    # Same filesystem as the destination permits atomic executable replacement.
    with tempfile.TemporaryDirectory(prefix=".enox-update-", dir=args.binary.parent) as temp:
        staged = stage_release(tag, arch, Path(temp))
        install(staged, args.binary, args.service, args.port)
    print(f"Installed {tag}; relay healthy and peer identity unchanged", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--service", default="enoxian-bootstrap")
    parser.add_argument("--port", type=int, default=36521)
    parser.add_argument("--binary", type=Path, default=Path("/usr/local/bin/enox"))
    parser.add_argument("--check", action="store_true", help="Report updates without installing")
    parser.add_argument("--updater", type=Path, default=Path(__file__).resolve(),
                        help="This updater's installed path, refreshed from each release")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("invalid HTTP port")
    def terminate(signum, frame):
        raise RuntimeError("Updater interrupted")
    signal.signal(signal.SIGTERM, terminate)
    with open("/run/lock/enoxian-relay-update.lock", "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print("Another relay update is running")
            return
        update(args)


if __name__ == "__main__":
    main()
