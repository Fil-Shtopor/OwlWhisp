"""Download every public release asset and verify the complete checksum set."""
import argparse
import json
import os
import shutil
import tempfile
import time
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

from package_checks import expected_assets, require, verify_checksums


def api(repo, endpoint):
    headers = {"User-Agent": "OwlWhisp-release-verifier", "Accept": "application/vnd.github+json"}
    if token := os.environ.get("GH_TOKEN"):
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(f"https://api.github.com/repos/{repo}/{endpoint}", headers=headers)
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def download(url, destination):
    # Asset requests deliberately carry no credentials: release downloads must be public.
    request = urllib.request.Request(url, headers={"User-Agent": "OwlWhisp-release-verifier"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=120) as response, destination.open("wb") as output:
                shutil.copyfileobj(response, output)
            return
        except (urllib.error.URLError, TimeoutError):
            if attempt == 2:
                raise
            time.sleep(5)


def verify(repo, tag, commit, version):
    require(tag == f"v{version}", "release tag does not match workspace version")
    release = api(repo, f"releases/tags/{tag}")
    require(not release["draft"], "release is still a draft")
    require(release["tag_name"] == tag, "wrong release tag")
    ref = api(repo, f"git/ref/tags/{tag}")["object"]
    for _ in range(5):
        if ref["type"] != "tag":
            break
        ref = api(repo, f"git/tags/{ref['sha']}")["object"]
    require(ref["type"] == "commit" and ref["sha"] == commit, "release tag points at a different commit")
    assets = release["assets"]
    names = [asset["name"] for asset in assets]
    require(len(names) == len(set(names)), "duplicate published asset names")
    require(set(names) == expected_assets(version), "published release has missing or unexpected assets")
    with tempfile.TemporaryDirectory(prefix="owlwhisp-release-downloads-") as temporary:
        directory = Path(temporary)
        for asset in assets:
            name = asset["name"]
            url = f"https://github.com/{repo}/releases/download/{tag}/{name}"
            require(asset["browser_download_url"] == url, f"unexpected download URL: {name}")
            destination = directory / name
            download(url, destination)
            require(destination.stat().st_size == asset["size"] > 0, f"download size mismatch: {name}")
            print(f"Public download verified: {name} ({asset['size']} bytes)", flush=True)
        verify_checksums(directory, version)
    print(f"Verified all {len(assets)} public assets and SHA-256 checksums for {repo} {tag}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default="Fil-Shtopor/OwlWhisp")
    parser.add_argument("--tag", required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    workspace = tomllib.loads((Path(__file__).resolve().parents[2] / "Cargo.toml").read_text(encoding="utf-8"))
    verify(args.repo, args.tag, args.commit, workspace["workspace"]["package"]["version"])
