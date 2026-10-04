"""Shared, dependency-free release package checks."""
import hashlib
import json
import struct
from collections import Counter
from pathlib import Path

TARGETS = {
    "win-arm64": ("aarch64-pc-windows-msvc", "windows-arm64", "arm64"),
    "win-x64": ("x86_64-pc-windows-msvc", "windows-x64", "x64"),
    "osx-arm64": ("aarch64-apple-darwin", "macos-arm64", "arm64"),
    "osx-x64": ("x86_64-apple-darwin", "macos-x64", "x64"),
    "linux-x64": ("x86_64-unknown-linux-gnu", "linux-x64", "x64"),
    "linux-arm64": ("aarch64-unknown-linux-gnu", "linux-arm64", "arm64"),
}


def verify_benchmark_audio(directory):
    """Installed benchmarks need real, licensed audio and nonempty reference transcripts."""
    manifest = directory / 'fixtures.json'
    require(manifest.is_file(), 'benchmark references are missing')
    clips = json.loads(manifest.read_text(encoding='utf-8'))
    require(Counter(c['language'] for c in clips) == Counter(dict.fromkeys(['en', 'es', 'ru', 'uk', 'zh'], 3)),
            'benchmark must contain three clips for each of five languages')
    require(len({c['file'] for c in clips}) == 15, 'duplicate benchmark audio')
    for clip in clips:
        require(bool(clip['transcript'].strip()), 'empty benchmark reference')
        require(clip['license'] == 'CC-BY-4.0' and bool(clip['attribution']), 'missing benchmark attribution')
        audio = directory / clip['file']
        require(audio.resolve().is_relative_to(directory.resolve()), 'benchmark path escapes bundle')
        require(audio.is_file(), f'missing benchmark audio: {clip["file"]}')
        original = Path(__file__).resolve().parents[2] / 'tests' / 'fixtures' / 'audio' / clip['file']
        require(original.is_file() and sha256(audio) == sha256(original),
                f'benchmark audio differs from the committed reference: {clip["file"]}')


def archive_name(platform, version):
    target = TARGETS[platform][0]
    if platform.startswith("win-"):
        return f"OwlWhisp-{version}-{target}.zip"
    if platform.startswith("osx-"):
        return f"OwlWhisp-{version}-{platform}-macos.zip"
    return f"OwlWhisp-{version}-{platform}.tar.gz"


def expected_assets(version):
    names = set()
    for platform, (target, job, _) in TARGETS.items():
        names.add(archive_name(platform, version))
        names.add(f"SHA256SUMS-{job}.txt")
        if platform.startswith("win-"):
            names.add(f"OwlWhisp-{version}-{target}-setup.exe")
    return names


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def binary_architecture(path):
    """Read the native executable header without loading the binary."""
    with Path(path).open("rb") as stream:
        header = stream.read(64)
        if header[:2] == b"MZ" and len(header) == 64:
            stream.seek(struct.unpack_from("<I", header, 60)[0])
            coff = stream.read(6)
            if coff[:4] != b"PE\0\0":
                raise ValueError(f"invalid PE header: {path}")
            return {0x8664: "x64", 0xAA64: "arm64", 0x14C: "x86"}.get(struct.unpack_from("<H", coff, 4)[0], "unknown")
        if header[:4] == b"\x7fELF" and len(header) >= 20:
            endian = "<" if header[5] == 1 else ">"
            machine = struct.unpack_from(endian + "H", header, 18)[0]
            if machine == 164:
                return "hexagon" # Qualcomm DSP skels are ELF32, not host ARM64 libraries.
            if header[4] != 2:
                raise ValueError(f"not a 64-bit ELF: {path}")
            return {62: "x64", 183: "arm64"}.get(machine, "unknown")
        if header[:4] == b"\xcf\xfa\xed\xfe" and len(header) >= 8:
            return {0x01000007: "x64", 0x0100000C: "arm64"}.get(struct.unpack_from("<I", header, 4)[0], "unknown")
    raise ValueError(f"unrecognized native binary: {path}")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_checksums(directory, version):
    directory = Path(directory)
    actual = {p.name for p in directory.iterdir() if p.is_file()}
    required = expected_assets(version)
    require(actual == required, f"wrong release files: missing={required - actual}, extra={actual - required}")
    covered = set()
    for platform, (_, job, _) in TARGETS.items():
        entries = []
        for line in (directory / f"SHA256SUMS-{job}.txt").read_text(encoding="utf-8-sig").splitlines():
            digest, name = line.split("  ", 1)
            require(Path(name).name == name, f"invalid checksum path: {name}")
            require(name not in covered, f"duplicate checksum: {name}")
            require((directory / name).is_file(), f"missing checksum target: {name}")
            require(sha256(directory / name) == digest.lower(), f"checksum mismatch: {name}")
            covered.add(name)
            entries.append(name)
        expected = {archive_name(platform, version)}
        if platform.startswith("win-"):
            expected.add(f"OwlWhisp-{version}-{TARGETS[platform][0]}-setup.exe")
        require(set(entries) == expected, f"wrong checksums for {platform}: {entries}")
    require(covered == {name for name in required if not name.startswith("SHA256SUMS-")}, "incomplete checksum coverage")
