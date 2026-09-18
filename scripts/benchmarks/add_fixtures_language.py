"""Add one language's clips to tests/fixtures/audio without disturbing the existing ones.

`make_fixtures_fleurs.py` rebuilds the whole fixture set, which would be the wrong tool here: every
measurement in models/catalog.json was taken over the current twelve clips, and silently swapping
one for another would invalidate all of them with nothing on screen to say so. This only appends.

Usage:
    python scripts/benchmarks/add_fixtures_language.py zh cmn_hans_cn

It refuses to touch a language that is already present, and it refuses to overwrite a wav.
"""

import csv
import json
import os
import shutil
import sys

import soundfile as sf

FLEURS = r"C:\Users\artam\lwdev\fleurs"
OUT = os.path.join(os.path.dirname(__file__), "..", "..", "tests", "fixtures", "audio")
OUT = os.path.abspath(OUT)
WANT = 3
MIN_S, MAX_S = 3.0, 8.0


def main(code: str, cfg: str) -> int:
    manifest_path = os.path.join(OUT, "fixtures.json")
    with open(manifest_path, encoding="utf-8") as fh:
        manifest = json.load(fh)

    if any(e["language"] == code for e in manifest):
        print(f"{code} is already in the fixture set; refusing to add it twice")
        return 1

    tsv = os.path.join(FLEURS, f"{cfg}.dev.tsv")
    rows = []
    with open(tsv, encoding="utf-8") as fh:
        for r in csv.reader(fh, delimiter="\t", quoting=csv.QUOTE_NONE):
            if len(r) >= 4:
                rows.append(r)

    picked, seen = [], set()
    for r in rows:
        if r[2].strip() in seen:
            continue
        p = os.path.join(FLEURS, cfg, "dev", r[1])
        if not os.path.exists(p):
            continue
        info = sf.info(p)
        dur = info.frames / info.samplerate
        if MIN_S <= dur <= MAX_S and info.samplerate == 16000:
            picked.append((r, p, dur))
            seen.add(r[2].strip())
        if len(picked) >= WANT:
            break

    if len(picked) < WANT:
        print(f"only found {len(picked)} usable clips for {code}; wanted {WANT}")
        return 1

    added = []
    for i, (r, p, dur) in enumerate(picked):
        name = f"fleurs_{code}_{i + 1}.wav"
        dest = os.path.join(OUT, name)
        if os.path.exists(dest):
            print(f"{name} already exists; refusing to overwrite it")
            return 1
        shutil.copyfile(p, dest)
        added.append(
            {
                "file": name,
                "language": code,
                "duration_s": round(dur, 2),
                "sample_rate": 16000,
                "transcript": r[2].strip(),
                "transcript_normalized": r[3].strip(),
                "source": f"google/fleurs {cfg} dev {r[1]}",
                "license": "CC-BY-4.0",
                "attribution": "FLEURS (Conneau et al., 2022), https://huggingface.co/datasets/google/fleurs",
            }
        )

    manifest.extend(added)
    with open(manifest_path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=2)
        fh.write("\n")

    total = sum(a["duration_s"] for a in added)
    print(f"added {len(added)} {code} clips ({total:.1f} s); fixture set is now {len(manifest)} clips")
    for a in added:
        print(f"  {a['file']}  {a['duration_s']}s  {a['transcript'][:40]}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__)
        raise SystemExit(2)
    raise SystemExit(main(sys.argv[1], sys.argv[2]))
