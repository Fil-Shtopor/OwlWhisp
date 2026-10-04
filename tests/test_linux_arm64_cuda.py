"""Pinned ARM64 native staging must reject corrupt or mismatched GPU packages."""
import hashlib
import importlib.util
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path

script = Path(__file__).resolve().parents[1] / "scripts/runtime/stage-linux-arm64-cuda.py"
spec = importlib.util.spec_from_file_location("arm64_cuda", script)
cuda = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cuda)


class Arm64CudaRuntime(unittest.TestCase):
    def wheel(self, path, wrong_arch=False, missing_notice=False):
        header = bytearray(64)
        header[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", header, 18, 62 if wrong_arch else 183)
        with zipfile.ZipFile(path, "w") as wheel:
            for source, name in cuda.FILES.items():
                if missing_notice and "ThirdPartyNotices" in source:
                    continue
                wheel.writestr(source, header if name.endswith(".so") else b"upstream licence")
            wheel.writestr("onnxruntime/capi/onnxruntime_pybind11_state.so", b"not shipped")
            wheel.writestr("../outside", b"not extracted")

    def test_only_matched_native_libraries_and_notices_are_staged(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, dest = root / "gpu.whl", root / "runtime"
            self.wheel(archive)
            cuda.stage(archive, dest)
            self.assertEqual({p.name for p in dest.iterdir()}, set(cuda.FILES.values()) | {"libonnxruntime.so.1"})
            self.assertEqual((dest / "libonnxruntime.so").read_bytes(), (dest / "libonnxruntime.so.1").read_bytes())
            self.assertFalse((root / "outside").exists())

    def test_wrong_architecture_and_missing_notices_preserve_existing_core(self):
        for kwargs in [dict(wrong_arch=True), dict(missing_notice=True)]:
            with self.subTest(**kwargs), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive, dest = root / "gpu.whl", root / "runtime"
                dest.mkdir()
                (dest / "libonnxruntime.so").write_bytes(b"existing core")
                self.wheel(archive, **kwargs)
                with self.assertRaises(ValueError):
                    cuda.stage(archive, dest)
                self.assertEqual((dest / "libonnxruntime.so").read_bytes(), b"existing core")
                self.assertEqual(len(list(dest.iterdir())), 1)

    def test_changed_download_is_rejected_by_size_or_sha256(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "gpu.whl"
            archive.write_bytes(b"reviewed bytes")
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            cuda.verify_archive(archive, size=14, digest=digest)
            archive.write_bytes(b"modified bytes")
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                cuda.verify_archive(archive, size=14, digest=digest)
            with self.assertRaisesRegex(ValueError, "size"):
                cuda.verify_archive(archive, size=15, digest=digest)


if __name__ == "__main__":
    unittest.main()
