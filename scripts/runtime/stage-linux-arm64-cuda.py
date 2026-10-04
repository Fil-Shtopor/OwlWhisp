"""Stage a pinned official ARM64 CUDA runtime; Python is only a build-time tool."""
import argparse
import hashlib
import os
import shutil
import struct
import tempfile
import urllib.request
import zipfile
from pathlib import Path

VERSION = "1.30.0"
URL = "https://files.pythonhosted.org/packages/54/21/81371bce6071c18a65c99f20a7943048146a46256aa34b318a21e057d75c/onnxruntime_gpu-1.30.0-cp311-cp311-manylinux_2_34_aarch64.whl"
SIZE = 205734826
SHA256 = "6c70b83afd6bfcc34b4b58d6a41f8c439c96811bbb7fe9fb1d2c2c9dd9c881b5"
FILES = {
    f"onnxruntime/capi/libonnxruntime.so.{VERSION}": "libonnxruntime.so",
    "onnxruntime/capi/libonnxruntime_providers_shared.so": "libonnxruntime_providers_shared.so",
    "onnxruntime/capi/libonnxruntime_providers_cuda.so": "libonnxruntime_providers_cuda.so",
    "onnxruntime/LICENSE": "onnxruntime-gpu-LICENSE.txt",
    "onnxruntime/ThirdPartyNotices.txt": "onnxruntime-gpu-ThirdPartyNotices.txt",
}


def verify_archive(archive, size=SIZE, digest=SHA256):
    if archive.stat().st_size != size:
        raise ValueError("ARM64 CUDA runtime archive size mismatch")
    with archive.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    if actual != digest:
        raise ValueError("ARM64 CUDA runtime archive SHA-256 mismatch")


def download(cache):
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"onnxruntime-gpu-{VERSION}-linux-arm64.whl"
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        with urllib.request.urlopen(URL, timeout=120) as response, partial.open("wb") as stream:
            shutil.copyfileobj(response, stream)
        verify_archive(partial)
        os.replace(partial, archive)
    verify_archive(archive)
    return archive


def stage(archive, destination):
    """Allowlist native C libraries and notices; never extract the Python bindings."""
    destination.mkdir(parents=True, exist_ok=True)
    # Validate all members before replacing any existing runtime. Core and providers must
    # always come from the same verified wheel, never a CPU core plus a separate GPU EP.
    with zipfile.ZipFile(archive) as wheel, tempfile.TemporaryDirectory(
        prefix=".cuda-stage-", dir=destination
    ) as temporary:
        staging = Path(temporary)
        for source, name in FILES.items():
            if wheel.namelist().count(source) != 1:
                raise ValueError(f"ARM64 CUDA wheel must contain exactly one {source}")
            path = staging / name
            with wheel.open(source) as stream, path.open("wb") as output:
                shutil.copyfileobj(stream, output)
            if name.endswith(".so"):
                with path.open("rb") as stream:
                    header = stream.read(64)
                if (header[:6] != b"\x7fELF\x02\x01" or len(header) != 64
                        or struct.unpack_from("<H", header, 18)[0] != 183):
                    raise ValueError(f"Not a Linux ARM64 ELF library: {source}")
            elif path.stat().st_size == 0:
                raise ValueError(f"Empty licence notice: {source}")
        for name in FILES.values():
            os.replace(staging / name, destination / name)
        # Refresh the SONAME alias too when switching an existing CPU-only staging tree.
        # Replacing a symlink does not overwrite its old CPU-library target.
        alias = staging / "libonnxruntime.so.1"
        if os.name == "nt":
            shutil.copyfile(destination / "libonnxruntime.so", alias)
        else:
            alias.symlink_to("libonnxruntime.so")
        os.replace(alias, destination / alias.name)
    print(f"Staged ONNX Runtime {VERSION} Linux ARM64 / CUDA 13 in {destination}")
    print("GPU inference needs a compatible NVIDIA driver, CUDA 13 libraries and cuDNN 9.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--archive", type=Path, help="Use an already downloaded, pinned wheel")
    args = parser.parse_args()
    archive = args.archive if args.archive else download(args.cache)
    verify_archive(archive)
    stage(archive, args.destination)
