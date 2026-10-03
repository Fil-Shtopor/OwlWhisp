import csv, os, json, shutil, soundfile as sf
F = r'C:\Users\artam\lwdev\fleurs'
OUT = r'C:\Users\artam\OneDrive\Desktop\OwlWhisp\tests\fixtures\audio'
os.makedirs(OUT, exist_ok=True)
langs = {'en': 'en_us', 'ru': 'ru_ru', 'es': 'es_419', 'uk': 'uk_ua'}
manifest = []
for code, cfg in langs.items():
    rows = []
    with open(os.path.join(F, f'{cfg}.dev.tsv'), encoding='utf-8') as fh:
        for r in csv.reader(fh, delimiter='\t', quoting=csv.QUOTE_NONE):
            if len(r) < 4: continue
            rows.append(r)
    picked = []
    seen = set()
    for r in rows:
        if r[2].strip() in seen: continue
        p = os.path.join(F, cfg, 'dev', r[1])
        if not os.path.exists(p): continue
        info = sf.info(p)
        dur = info.frames / info.samplerate
        if 3.0 <= dur <= 8.0 and info.samplerate == 16000:
            picked.append((r, p, dur)); seen.add(r[2].strip())
        if len(picked) >= 3: break
    for i, (r, p, dur) in enumerate(picked):
        name = f'fleurs_{code}_{i+1}.wav'
        shutil.copyfile(p, os.path.join(OUT, name))
        manifest.append({'file': name, 'language': code, 'duration_s': round(dur, 2), 'sample_rate': 16000,
                         'transcript': r[2].strip(), 'transcript_normalized': r[3].strip(),
                         'source': f'google/fleurs {cfg} dev {r[1]}', 'license': 'CC-BY-4.0',
                         'attribution': 'FLEURS (Conneau et al., 2022), https://huggingface.co/datasets/google/fleurs'})
        print(code, name, f'{dur:.1f}s', r[2][:70])
json.dump(manifest, open(os.path.join(OUT, 'fixtures.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=2)
print('total', len(manifest))
