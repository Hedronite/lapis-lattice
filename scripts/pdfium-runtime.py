#!/usr/bin/env python3
"""Prepare the pinned PDF runtime and all upstream notices for desktop packaging.

This is an explicit packaging step, never invoked when a user opens a PDF.
The destination must be new; existing app/runtime installations are not modified.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import urllib.request

RELEASE = "chromium/7881"
HASHES = {
    "mac-arm64": "52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40",
    "mac-x64": "6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b",
    "linux-x64": "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
    "linux-arm64": "ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674",
}
MAX_ARCHIVE = 64 * 1024 * 1024


def prepare(platform, destination, archive=None):
    asset = f"pdfium-{platform}.tgz"
    url = f"https://github.com/bblanchon/pdfium-binaries/releases/download/{RELEASE}/{asset}"
    destination = Path(destination).absolute()
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"Destination already exists: {destination}")
    if archive:
        with Path(archive).open("rb") as source:
            data = source.read(MAX_ARCHIVE + 1)
    else:
        with urllib.request.urlopen(url, timeout=60) as source:
            data = source.read(MAX_ARCHIVE + 1)
    if len(data) > MAX_ARCHIVE:
        raise ValueError("Runtime archive exceeds download budget")
    digest = hashlib.sha256(data).hexdigest()
    if digest != HASHES[platform]:
        raise ValueError(f"Runtime checksum mismatch: expected {HASHES[platform]}, got {digest}")
    library = "libpdfium.dylib" if platform.startswith("mac-") else "libpdfium.so"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pdfium-stage-", dir=destination.parent) as temporary:
        stage = Path(temporary) / "runtime"
        stage.mkdir()
        files = {}
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as bundle:
            for member in bundle:
                name = PurePosixPath(member.name)
                if name.is_absolute() or ".." in name.parts:
                    raise ValueError("Unsafe runtime archive path")
                relative = name.as_posix()
                if relative == f"lib/{library}":
                    output = Path(library)
                elif relative == "LICENSE":
                    output = Path("licenses/pdfium-binaries-MIT.txt")
                elif relative.startswith("licenses/") and member.isfile():
                    output = Path(relative)
                else:
                    continue
                if not member.isfile() or member.size > MAX_ARCHIVE:
                    raise ValueError("Invalid runtime archive member")
                target = stage / output
                target.parent.mkdir(parents=True, exist_ok=True)
                with bundle.extractfile(member) as source, target.open("xb") as sink:
                    shutil.copyfileobj(source, sink)
                files[output.as_posix()] = hashlib.sha256(target.read_bytes()).hexdigest()
        for required in (library, "licenses/pdfium.txt", "licenses/pdfium-binaries-MIT.txt"):
            if required not in files:
                raise ValueError(f"Runtime archive is missing {required}")
        manifest = {"release": RELEASE, "platform": platform, "source": url,
                    "archive_sha256": digest, "files": files}
        (stage / "pdfium-runtime.json").write_text(json.dumps(manifest, indent=2) + "\n")
        # mkdir reserves the destination without replacing even an empty directory.
        destination.mkdir()
        for source in stage.iterdir():
            shutil.move(str(source), destination / source.name)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=HASHES)
    parser.add_argument("--destination", required=True, type=Path)
    parser.add_argument("--archive", type=Path, help="Use an already downloaded archive; checksum is still required")
    args = parser.parse_args()
    try:
        result = prepare(args.platform, args.destination, args.archive)
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(1, f"pdfium-runtime: {error}\n")
    print(json.dumps(result, indent=2))
