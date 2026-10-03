"""Exercise installed runtime workers on real speech; no user settings are modified."""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--exe', type=Path, required=True)
parser.add_argument('--data', type=Path, required=True)
parser.add_argument('--model', type=Path, required=True)
parser.add_argument('--audio', type=Path, required=True)
args = parser.parse_args()
wav = args.audio.read_bytes()
assert wav[:4] == b'RIFF' and wav[8:12] == b'WAVE'
offset = 12
fmt = pcm = None
while offset + 8 <= len(wav):
    kind, size = struct.unpack_from('<4sI', wav, offset)
    chunk = wav[offset+8:offset+8+size]
    if kind == b'fmt ': fmt = chunk
    if kind == b'data': pcm = chunk
    offset += 8 + size + (size % 2)
tag, channels, rate, _, _, bits = struct.unpack_from('<HHIIHH', fmt)
assert channels == 1 and pcm is not None
if tag == 3 and bits == 32:
    payload = pcm
elif tag == 1 and bits == 16:
    payload = b''.join(struct.pack('<f', sample/32768) for (sample,) in struct.iter_unpack('<h', pcm))
else:
    raise ValueError(f'unsupported WAV format {tag}/{bits}')
sample_count = len(payload)//4
env = os.environ.copy()
env['LW_APP_DATA_DIR'] = str(args.data.resolve())
results = []
for accel in ['cuda', 'direct_ml', 'web_gpu', 'tensor_rt']:
    runtime = next((args.data / 'runtimes/win-x64').glob(accel + '-*'))
    start = time.perf_counter()
    worker = subprocess.Popen([str(args.exe.resolve()), '--provider-worker', str(runtime.resolve()),
                               str(args.model.resolve()), str((args.data / 'model-cache').resolve()), '0', accel],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    try:
        ready = json.loads(worker.stdout.readline())
        assert ready['Ok']['accelerator'] == accel, ready
        print(accel, 'ready in', round(time.perf_counter()-start,2), 's', flush=True)
        runs = []
        for _ in range(2):
            start = time.perf_counter()
            worker.stdin.write(bytes([2]) + struct.pack('<II',sample_count,rate) + payload)
            worker.stdin.flush()
            answer = json.loads(worker.stdout.readline())
            assert 'Ok' in answer and answer['Ok']['text'].strip(), answer
            runs.append({'seconds':time.perf_counter()-start,'text':answer['Ok']['text']})
        results.append({'accelerator':accel,'device':ready['Ok']['device'],'runs':runs})
        print(accel, 'speech:', runs[-1]['text'], 'warm seconds:',round(runs[-1]['seconds'],3),flush=True)
        worker.stdin.write(bytes([3])); worker.stdin.flush()
        worker.wait(timeout=30)
    finally:
        if worker.poll() is None: worker.kill(); worker.wait()
assert len({result['runs'][-1]['text'] for result in results}) == 1, results
(args.data / 'speech-check.json').write_text(json.dumps(results,indent=2,ensure_ascii=False),encoding='utf-8')
print('All four runtimes used their requested provider and produced the same speech text.',flush=True)
