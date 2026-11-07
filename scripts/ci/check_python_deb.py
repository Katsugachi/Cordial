"""Inspect an experimental Debian bundle without extracting paths from it."""
from pathlib import Path
import hashlib
import subprocess
import tarfile

files = list(Path("target/debug/bundle/deb").glob("*.deb"))
if len(files) != 1:
    raise SystemExit(f"expected exactly one debug Debian bundle, found {len(files)}")
wanted = {
    "python/python.wasm": "e5dc5a398b07b54ea8fdb503bf68fb583d533f10ec3f930963e02b9505f7a763",
    "python/licenses/python-3.12.txt": None,
    "python/licenses/python-wasi-apache-2.0.txt": None,
    "python/licenses/wasmtime-apache-with-llvm-exception.txt": None,
}
found = set()
proc = subprocess.Popen(["dpkg-deb", "--fsys-tarfile", str(files[0])], stdout=subprocess.PIPE)
assert proc.stdout is not None
with tarfile.open(fileobj=proc.stdout, mode="r|") as archive:
    for member in archive:
        for destination, expected in wanted.items():
            if member.name.endswith("/" + destination):
                if destination in found or not member.isfile():
                    raise SystemExit(f"duplicate or non-regular resource: {member.name}")
                resource = archive.extractfile(member)
                if resource is None:
                    raise SystemExit(f"missing resource contents: {member.name}")
                digest = hashlib.file_digest(resource, "sha256").hexdigest()
                if expected is not None and digest != expected:
                    raise SystemExit(f"bad digest for {member.name}")
                found.add(destination)
if proc.wait() != 0:
    raise SystemExit("dpkg-deb failed")
if found != set(wanted):
    raise SystemExit(f"missing bundled resources: {sorted(set(wanted) - found)}")
print(f"Verified experimental resources in {files[0]}")
