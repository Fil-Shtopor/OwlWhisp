"""Reuse the unchanged ORT 1.28.1 Intel library from a verified OwlWhisp release."""
import argparse
import hashlib
import os
import shutil
import struct
import tempfile
import urllib.request
import zipfile
from pathlib import Path

URL = "https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.2/OwlWhisp-0.1.2-osx-x64-macos.zip"
SHA256 = "adda7637a65003ad361c6d07d3de70b7b033bbbe8e815b79f02d1b741c283fb8"
PREFIX = "OwlWhisp-0.1.2-osx-x64.app/Contents/MacOS/runtime/osx-x64/"
FILES = ["libonnxruntime.dylib", "libonnxruntime.1.dylib", "libonnxruntime.1.28.1.dylib",
         "onnxruntime-LICENSE", "onnxruntime-ThirdPartyNotices.txt"]


def stage(archive, destination):
    with archive.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != SHA256:
            raise ValueError("Pinned Intel ONNX Runtime cache SHA-256 mismatch")
    destination.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as bundle, tempfile.TemporaryDirectory(
        prefix=".ort-cache-", dir=destination
    ) as temporary:
        staging = Path(temporary)
        for name in FILES:
            source = PREFIX + name
            if bundle.namelist().count(source) != 1:
                raise ValueError(f"Pinned Intel runtime cache is missing {source}")
            path = staging / name
            with bundle.open(source) as stream, path.open("wb") as output:
                shutil.copyfileobj(stream, output)
            if name.endswith(".dylib"):
                with path.open("rb") as stream:
                    header = stream.read(8)
                if header[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", header, 4)[0] != 0x01000007:
                    raise ValueError(f"Not an Intel macOS library: {name}")
        for name in FILES:
            os.replace(staging / name, destination / name)
    print("Reused verified ONNX Runtime 1.28.1 native Intel macOS library")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    if args.archive:
        stage(args.archive, args.destination)
    else:
        with tempfile.TemporaryDirectory(prefix="owlwhisp-ort-download-") as temporary:
            archive = Path(temporary) / "runtime.zip"
            with urllib.request.urlopen(URL, timeout=120) as response, archive.open("wb") as output:
                shutil.copyfileobj(response, output)
            stage(archive, args.destination)
