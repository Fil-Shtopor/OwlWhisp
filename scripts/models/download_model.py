#!/usr/bin/env python3
"""Dev helper: download and SHA-256-verify a LocalWisper model from a manifest.

This is a *development* convenience mirroring the Rust `lw_core::model::ModelDownloader`. The
shipping app never requires Python — it downloads models itself. Use this to pre-populate a local
model directory for testing.

    python download_model.py models/manifests/parakeet-tdt-0.6b-v3.json --dest <dir> [--target cpu_int8]

Only stdlib is used.
"""
import argparse
import hashlib
import json
import os
import sys
import urllib.request

CHUNK = 1024 * 1024


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            b = f.read(CHUNK)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def fetch(url, dest, expected_bytes, expected_sha):
    tmp = dest + ".part"
    have = os.path.getsize(tmp) if os.path.exists(tmp) else 0
    req = urllib.request.Request(url)
    if have:
        req.add_header("Range", f"bytes={have}-")
    mode = "ab" if have else "wb"
    with urllib.request.urlopen(req, timeout=120) as r, open(tmp, mode) as out:
        downloaded = have
        while True:
            chunk = r.read(CHUNK)
            if not chunk:
                break
            out.write(chunk)
            downloaded += len(chunk)
            pct = 100.0 * downloaded / expected_bytes if expected_bytes else 0
            print(f"\r  {os.path.basename(dest)}: {downloaded/1e6:8.1f} MB ({pct:5.1f}%)", end="")
    print()
    actual = sha256(tmp)
    if actual.lower() != expected_sha.lower():
        os.remove(tmp)
        raise SystemExit(f"SHA-256 mismatch for {dest}: expected {expected_sha}, got {actual}")
    if os.path.getsize(tmp) != expected_bytes:
        os.remove(tmp)
        raise SystemExit(f"size mismatch for {dest}")
    os.replace(tmp, dest)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("manifest")
    ap.add_argument("--dest", required=True, help="destination model directory")
    ap.add_argument("--target", default="cpu_int8", help="artifact target key (e.g. cpu_int8, qnn_htp_v81)")
    args = ap.parse_args()

    with open(args.manifest, encoding="utf-8") as f:
        m = json.load(f)
    if not str(m.get("url", m.get("common", [{}])[0].get("url", ""))).startswith("https://"):
        # Every file must be https; validated per-file below too.
        pass

    dest = os.path.join(args.dest, m["local_dir"])
    os.makedirs(dest, exist_ok=True)
    files = list(m.get("common", []))
    chosen = None
    for a in m.get("artifacts", []):
        if a["target"] == args.target:
            chosen = a
            files += a["files"]
            break
    if chosen is None:
        raise SystemExit(f"target '{args.target}' not in manifest (have: {[a['target'] for a in m['artifacts']]})")

    print(f"model {m['id']} v{m['version']} ({m['license']}) -> {dest}")
    for fe in files:
        if not fe["url"].startswith("https://"):
            raise SystemExit(f"refusing non-https url: {fe['url']}")
        out = os.path.join(dest, fe["path"])
        os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
        if os.path.exists(out) and os.path.getsize(out) == fe["bytes"] and sha256(out).lower() == fe["sha256"].lower():
            print(f"  {fe['path']}: already present, verified")
            continue
        fetch(fe["url"], out, fe["bytes"], fe["sha256"])
    print(f"done. {m['attribution']}")


if __name__ == "__main__":
    main()
