"""Summarize compare-tensorrt.ps1 logs without mixing inference and session build times."""
import argparse
import json
import re
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('results', type=Path)
args = parser.parse_args()
rows = []
for path in sorted(args.results.glob('*/*.txt')):
    if '.stderr.' in path.name:
        continue
    if not (path.name.startswith('run-') or path.name.startswith('prepared-')):
        continue
    log = path.read_text(encoding='utf-8-sig', errors='replace')
    warm = re.search(r'warm RTF: ([\d.]+)\s+cold RTF: ([\d.]+)', log)
    wer = re.search(r'token-weighted WER: ([\d.]+)', log)
    load = re.search(r'engine load \(sessions\): ([\d.]+) ms', log)
    if not (warm and wer):
        continue
    metadata = json.loads(path.with_suffix('.json').read_text(encoding='utf-8-sig'))
    rows.append({**metadata, 'warm_rtf': float(warm[1]), 'first_clip_rtf': float(warm[2]),
                 'wer': float(wer[1]), 'engine_load_seconds': float(load[1]) / 1000 if load else None,
                 'source': str(path)})
print('| Profile | Run | Shapes prepared | Engine load, s | First clip RTF | Later clips RTF | WER | Cache, GB |')
print('|---|---:|---|---:|---:|---:|---:|---:|')
for row in rows:
    load = f"{row['engine_load_seconds']:.2f}" if row['engine_load_seconds'] is not None else '?'
    cache = f"{row['cache_bytes'] / 1e9:.2f}" if row['cache_bytes'] else '0'
    prepared = 'yes' if row.get('all_shapes_prepared') else 'no'
    print(f"| {row['profile']} | {row['run']} | {prepared} | {load} | {row['first_clip_rtf']:.4f} | "
          f"{row['warm_rtf']:.4f} | {row['wer']:.3f} | {cache} |")
(args.results / 'summary.json').write_text(json.dumps(rows, indent=2), encoding='utf-8')
