"""Release gates must reject corrupt, incomplete and incorrectly labelled artifacts."""
import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts" / "build"))
from package_checks import TARGETS, archive_name, binary_architecture, expected_assets, sha256, verify_checksums


class ReleasePackaging(unittest.TestCase):
    def test_native_headers_distinguish_x64_arm64_and_hexagon(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "binary"
            for machine, expected in [(0x8664, "x64"), (0xAA64, "arm64")]:
                data = bytearray(70)
                data[:2] = b"MZ"
                struct.pack_into("<I", data, 60, 64)
                data[64:68] = b"PE\0\0"
                struct.pack_into("<H", data, 68, machine)
                path.write_bytes(data)
                self.assertEqual(binary_architecture(path), expected)
            for machine, cls, expected in [(62, 2, "x64"), (183, 2, "arm64"), (164, 1, "hexagon")]:
                data = bytearray(64)
                data[:4] = b"\x7fELF"
                data[4:6] = bytes([cls, 1])
                struct.pack_into("<H", data, 18, machine)
                path.write_bytes(data)
                self.assertEqual(binary_architecture(path), expected)
            for machine, expected in [(0x01000007, "x64"), (0x0100000C, "arm64")]:
                path.write_bytes(b"\xcf\xfa\xed\xfe" + struct.pack("<I", machine))
                self.assertEqual(binary_architecture(path), expected)
            path.write_bytes(b"not a binary")
            with self.assertRaises(ValueError):
                binary_architecture(path)

    def create_assets(self, directory):
        for name in expected_assets("0.1.0"):
            if not name.startswith("SHA256SUMS-"):
                (directory / name).write_bytes(name.encode())
        for platform, (target, job, _) in TARGETS.items():
            names = [archive_name(platform, "0.1.0")]
            if platform.startswith("win-"):
                names.append(f"OwlWhisp-0.1.0-{target}-setup.exe")
            (directory / f"SHA256SUMS-{job}.txt").write_text("".join(f"{sha256(directory / name)}  {name}\n" for name in names))

    def test_all_six_platforms_and_fourteen_assets_are_required(self):
        self.assertEqual(len(expected_assets("0.1.0")), 14)
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            self.create_assets(directory)
            verify_checksums(directory, "0.1.0")
            (directory / archive_name("linux-arm64", "0.1.0")).unlink()
            with self.assertRaisesRegex(ValueError, "missing"):
                verify_checksums(directory, "0.1.0")

    def test_corrupt_package_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            self.create_assets(directory)
            (directory / archive_name("win-x64", "0.1.0")).write_bytes(b"corrupted")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                verify_checksums(directory, "0.1.0")

    def test_wrong_version_and_extra_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            self.create_assets(directory)
            with self.assertRaises(ValueError):
                verify_checksums(directory, "0.1.1")
            (directory / "old-package.zip").write_bytes(b"stale")
            with self.assertRaisesRegex(ValueError, "extra"):
                verify_checksums(directory, "0.1.0")


if __name__ == "__main__":
    unittest.main()
