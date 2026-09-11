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
import tarfile
import tempfile
import time
import urllib.request

RELEASE_API = "https://api.github.com/repos/suzent/enoxian/releases/latest"
RELEASE_BASE = "https://github.com/suzent/enoxian/releases/download"


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "enoxian-relay-updater"})
    return urllib.request.urlopen(request, timeout=30)


def version(text):
    match = re.fullmatch(r"(?:enox |v)?(\d+)\.(\d+)\.(\d+)", text.strip())
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


def stage_release(tag, arch, directory):
    asset = f"enoxian-linux-{arch}.tar.gz"
    base = f"{RELEASE_BASE}/{tag}"
    with fetch(base + "/SHA256SUMS") as response:
        lines = response.read().decode("utf-8").splitlines()
    hashes = [line.split()[0] for line in lines
              if len(line.split()) == 2 and line.split()[1].lstrip("*") == asset]
    if len(hashes) != 1 or not re.fullmatch(r"[0-9a-fA-F]{64}", hashes[0]):
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
    if digest.hexdigest() != hashes[0].lower():
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
