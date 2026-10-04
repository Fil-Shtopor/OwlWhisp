"""Verify a release archive and optionally load its runtime on the native build host."""
import argparse
import json
import os
import plistlib
import re
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path

from package_checks import TARGETS, archive_name, binary_architecture, require
from windows_resources import verify_branding


def unpack(archive, directory):
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            for member in bundle.infolist():
                resolved = (directory / member.filename).resolve()
                require(resolved.is_relative_to(directory.resolve()), "ZIP path escapes extraction directory")
            bundle.extractall(directory)
            for member in bundle.infolist():
                mode = (member.external_attr >> 16) & 0o777
                if mode and os.name != "nt":
                    os.chmod(directory / member.filename, mode)
    else:
        with tarfile.open(archive) as bundle:
            bundle.extractall(directory, filter="data")


def verify(archive, platform, target, version, commit, smoke, allow_no_sherpa=False):
    require(target == TARGETS[platform][0], f"target/platform mismatch: {target}/{platform}")
    require(archive.is_file() and archive.stat().st_size > 0, f"missing package: {archive}")
    with tempfile.TemporaryDirectory(prefix="owlwhisp-package-") as temporary:
        directory = Path(temporary)
        unpack(archive, directory)
        if platform.startswith("win-"):
            executable = directory / "OwlWhisp.exe"
            resources = directory
            runtime = directory / "runtime" / platform
        elif platform.startswith("osx-"):
            apps = list(directory.glob("*.app"))
            require(len(apps) == 1, "archive must contain one .app bundle")
            contents = apps[0] / "Contents"
            resources = contents / "Resources"
            executable = contents / "MacOS" / "OwlWhisp"
            runtime = contents / "MacOS" / "runtime" / platform
            plist = plistlib.loads((contents / "Info.plist").read_bytes())
            require(plist["CFBundleExecutable"] == executable.name, "wrong bundle executable")
            require(plist["CFBundleShortVersionString"] == version, "wrong bundle version")
        else:
            root = directory / f"OwlWhisp-{version}-{platform}"
            resources = root
            executable = root / "owlwhisp"
            runtime = root / "runtime"
        for name in ["LICENSE", "LICENSES.md", "THIRD_PARTY_NOTICES.md", "BUILD_INFO.json"]:
            require((resources / name).is_file(), f"missing {name}")
        info = json.loads((resources / "BUILD_INFO.json").read_text(encoding="utf-8-sig"))
        require(info["version"] == version and info["platform"] == platform and info["target"] == target, f"wrong build metadata: {info}")
        require(info["commit"] == commit, f"package is from another commit: {info}")
        if not allow_no_sherpa:
            require(info["sherpa"] == (platform != "linux-arm64"), "wrong portable engine feature set")
        require(binary_architecture(executable) == TARGETS[platform][2], "wrong application architecture")
        if platform.startswith('win-'):
            verify_branding(executable, Path('assets/icons/icon.ico'))
            rtf = (resources / 'LICENSES.rtf').read_bytes()
            require(rtf.startswith(b'{\\rtf1') and b'\\trowd' in rtf and b'\\u' in rtf, 'Installer licence is not a Unicode RTF document with tables')
            installer = archive.with_name(f'OwlWhisp-{version}-{target}-setup.exe')
            if installer.exists():
                verify_branding(installer, Path('assets/icons/icon.ico'))
        if os.name != "nt":
            require(executable.stat().st_mode & 0o111, "archive lost executable permission")
        library = "onnxruntime.dll" if platform.startswith("win-") else "libonnxruntime.dylib" if platform.startswith("osx-") else "libonnxruntime.so"
        require((runtime / library).is_file(), "runtime core is missing")
        for binary in runtime.rglob("*"):
            if binary.suffix in [".dll", ".so", ".dylib"]:
                expected = "hexagon" if re.fullmatch(r"libQnnHtpV\d+Skel\.so", binary.name) else TARGETS[platform][2]
                require(binary_architecture(binary) == expected, f"wrong native architecture: {binary.name}")
        if platform != "win-arm64":
            require(not any(p.name.startswith(("Qnn", "libQnn")) for p in directory.rglob("*")), "Qualcomm runtime in a prohibited package")
        else:
            for name in ["onnxruntime_providers_qnn.dll", "QnnHtp.dll", "QnnSystem.dll", "QnnHtpV81Stub.dll", "libQnnHtpV81Skel.so"]:
                require((runtime / name).is_file(), f"missing Qualcomm runtime: {name}")
        if platform == "win-x64":
            for name in ["onnxruntime_providers_webgpu.dll", "dxcompiler.dll", "dxil.dll"]:
                require((runtime / name).is_file(), f"missing portable GPU component: {name}")
            directml = runtime.parent / "win-x64-directml"
            for name in ["onnxruntime.dll", "DirectML.dll", "Microsoft.Windows.AI.MachineLearning.dll"]:
                require((directml / name).is_file(), f"missing DirectML component: {name}")
                require(binary_architecture(directml / name) == "x64", f"wrong DirectML architecture: {name}")
            require(not any(runtime.glob("nvinfer*")) and not any(runtime.glob("cublas*")), "base package unexpectedly contains NVIDIA add-ons")
        manifests = executable.parent / "models" / "manifests"
        require(any(manifests.glob("*.json")), "model manifests are missing")
        if info["sherpa"]:
            data = executable.read_bytes().lower()
            require(b"espeak-ng" not in data and b"piper_phonemize" not in data, "TTS component in application binary")
        if smoke:
            env = os.environ.copy()
            env["LW_RUNTIME_DIR"] = str(runtime)
            # Keep downloads, cache and settings isolated from any real user profile.
            env["APPDATA"] = str(directory / "app-data")
            env["XDG_DATA_HOME"] = str(directory / "app-data")
            result = subprocess.run([str(executable), "--diagnose-accelerators"], cwd=executable.parent,
                                    env=env, capture_output=True, text=True, timeout=120)
            require(result.returncode == 0, f"packaged diagnostic failed: {result.stderr[-2000:]}")
            report = json.loads(result.stdout)
            require(report["runtime_error"] is None, f"packaged runtime did not load: {report['runtime_error']}")
            # Rust canonicalizes Windows paths with the extended-length prefix (\\?\).
            # Compare directory identity so equivalent native path spellings remain valid.
            require(Path(report["runtime_dir"]).samefile(runtime), "diagnostic loaded a runtime outside the package")
            require(any(a["id"] == "cpu" and a["usable"] for a in report["accelerators"]), "packaged CPU provider is unusable")
            require(report["sherpa_enabled"] == info["sherpa"], "metadata disagrees with compiled engine features")
            print(f"Native packaged runtime loaded: {report['os']} / {report['arch']}")
    print(f"Verified {archive.name}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=TARGETS, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--allow-no-sherpa", action="store_true", help="For local Parakeet-only development packages")
    parser.add_argument("--directory", type=Path, default=Path("dist"))
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    verify(args.directory / archive_name(args.platform, version), args.platform, args.target, version, args.commit, args.smoke, args.allow_no_sherpa)
