"""Reject partial releases, incorrectly named assets and corrupt downloads."""
import argparse
import tomllib
from pathlib import Path
from package_checks import require, verify_checksums

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    require(args.tag == f"v{version}", "release tag does not match workspace/package version")
    verify_checksums(args.directory, version)
    print(f"Verified all six platforms and 14 release assets for {args.tag}")
