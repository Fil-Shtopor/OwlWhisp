"""Reference end-to-end Parakeet TDT 0.6B v3 pipeline in Python (spec for the Rust implementation).

  wav (16 kHz) -> nemo128.onnx (mel 128) -> encoder (CPU fp32 dynamic | CPU static | HTP EPContext) -> TDT greedy decode
  with the istupakov decoder_joint model -> SentencePiece detokenization -> text; WER vs FLEURS transcripts.

Usage: python tdt_reference.py [--encoder cpu|static|htp] [--frames 1000] [--fixtures DIR]
"""
import argparse, os, sys, time, json, re, unicodedata
import numpy as np
import onnxruntime as ort
import soundfile as sf

M = r'C:\Users\artam\AppData\Local\Temp\claude\C--Users-artam-OneDrive-Desktop-OwlWhisp\99cce259-4884-4ea7-a3a6-ddeeafec6e91\scratchpad\models'
IST = os.path.join(M, 'istupakov')
WORK = r'C:\Users\artam\lwdev\work'
LIBDIR = r'C:\Users\artam\lwdev\venv\Lib\site-packages\onnxruntime_qnn\libs\arm64ec'
BLANK = 8192           # vocab size 8192 + blank
NUM_TOKENS = 8193
DURATIONS = [0, 1, 2, 3, 4]
MAX_SYMBOLS_PER_FRAME = 10

ap = argparse.ArgumentParser()
ap.add_argument('--encoder', default='cpu', choices=['cpu', 'static', 'htp'])
ap.add_argument('--frames', type=int, default=1000)
ap.add_argument('--fixtures', default=r'C:\Users\artam\OneDrive\Desktop\OwlWhisp\tests\fixtures\audio')
ap.add_argument('--decoder', default=os.path.join(IST, 'decoder_joint-model.int8.onnx'))
args = ap.parse_args()
T = args.frames

def log(*a): print(*a, flush=True)

# ---------- vocab
vocab = []
with open(os.path.join(IST, 'vocab.txt'), encoding='utf-8') as fh:
    for line in fh:
        line = line.rstrip('\n')
        if not line: continue
        parts = line.split(' ')
        vocab.append(parts[0] if len(parts) == 2 and parts[1].isdigit() else line)
log(f'vocab entries: {len(vocab)} (first: {vocab[:3]!r}, last: {vocab[-2:]!r})')

def detok(ids):
    s = ''.join(vocab[i] for i in ids)
    return s.replace('\u2581', ' ').strip()

# ---------- sessions
pre = ort.InferenceSession(os.path.join(IST, 'nemo128.onnx'), providers=['CPUExecutionProvider'])
dec = ort.InferenceSession(args.decoder, providers=['CPUExecutionProvider'])
dec_in = [i.name for i in dec.get_inputs()]; dec_out = [o.name for o in dec.get_outputs()]
log('decoder_joint inputs', dec_in, 'outputs', dec_out)

t0 = time.perf_counter()
if args.encoder == 'cpu':
    enc = ort.InferenceSession(os.path.join(IST, 'encoder-model.onnx'), providers=['CPUExecutionProvider'])
elif args.encoder == 'static':
    enc = ort.InferenceSession(os.path.join(WORK, f'encoder_static_b1_t{T}.onnx'), providers=['CPUExecutionProvider'])
else:
    os.add_dll_directory(LIBDIR); os.environ['PATH'] = LIBDIR + os.pathsep + os.environ['PATH']
    ort.register_execution_provider_library('QNNExecutionProvider', os.path.join(LIBDIR, 'onnxruntime_providers_qnn.dll'))
    npu = [d for d in ort.get_ep_devices() if d.ep_name == 'QNNExecutionProvider' and d.device.type == ort.OrtHardwareDeviceType.NPU]
    so = ort.SessionOptions(); so.add_provider_for_devices(npu, {'backend_type': 'htp', 'htp_performance_mode': 'burst'})
    enc = ort.InferenceSession(os.path.join(WORK, f'encoder_fp16_t{T}_ctx.onnx'), so)
log(f'encoder [{args.encoder}] loaded in {time.perf_counter()-t0:.2f}s; providers={enc.get_providers()}')

def run_encoder(feats, n):
    if args.encoder == 'cpu':
        out, out_len = enc.run(None, {'audio_signal': feats[:, :, :n], 'length': np.array([n], dtype=np.int64)})
    else:
        x = np.zeros((1, 128, T), dtype=np.float32); x[:, :, :min(n, T)] = feats[:, :, :min(n, T)]
        out, out_len = enc.run(None, {'audio_signal': x, 'length': np.array([min(n, T)], dtype=np.int64)})
    return out, int(out_len[0])

def tdt_greedy(enc_out, enc_len):
    """enc_out: [1, 1024, T']. Returns (token ids, frame indices)."""
    h = np.zeros((2, 1, 640), dtype=np.float32); c = np.zeros((2, 1, 640), dtype=np.float32)
    last = BLANK; tokens = []; frames = []
    t = 0; steps = 0
    while t < enc_len:
        f = np.ascontiguousarray(enc_out[:, :, t:t + 1])
        symbols = 0
        while True:
            steps += 1
            logits, _, h_new, c_new = dec.run(None, {'encoder_outputs': f, 'targets': np.array([[last]], dtype=np.int32),
                                                     'target_length': np.array([1], dtype=np.int32), 'input_states_1': h, 'input_states_2': c})
            v = logits.reshape(-1)
            tok = int(np.argmax(v[:NUM_TOKENS])); dur = DURATIONS[int(np.argmax(v[NUM_TOKENS:]))]
            if tok == BLANK:
                t += max(dur, 1)          # blank never stalls
                break
            tokens.append(tok); frames.append(t); last = tok; h, c = h_new, c_new; symbols += 1
            t += dur
            if dur > 0: break              # moved on to a new frame
            if symbols >= MAX_SYMBOLS_PER_FRAME:
                t += 1; break
    return tokens, frames, steps

def norm(s):
    s = unicodedata.normalize('NFKC', s).lower()
    s = re.sub(r"[^\w\s'’-]", ' ', s)
    return re.sub(r'\s+', ' ', s).strip()

def wer(ref, hyp):
    r, h = ref.split(), hyp.split()
    d = np.zeros((len(r) + 1, len(h) + 1), dtype=np.int32)
    d[:, 0] = np.arange(len(r) + 1); d[0, :] = np.arange(len(h) + 1)
    for i in range(1, len(r) + 1):
        for j in range(1, len(h) + 1):
            d[i, j] = min(d[i - 1, j] + 1, d[i, j - 1] + 1, d[i - 1, j - 1] + (r[i - 1] != h[j - 1]))
    return d[len(r), len(h)] / max(1, len(r))

fixtures = json.load(open(os.path.join(args.fixtures, 'fixtures.json'), encoding='utf-8'))
tot_err = 0; tot_words = 0; per_lang = {}
for fx in fixtures:
    wav, sr = sf.read(os.path.join(args.fixtures, fx['file']), dtype='float32')
    if wav.ndim > 1: wav = wav.mean(axis=1)
    assert sr == 16000
    t0 = time.perf_counter()
    feats, flen = pre.run(None, {'waveforms': wav[None, :], 'waveforms_lens': np.array([len(wav)], dtype=np.int64)})
    t_feat = time.perf_counter() - t0
    t0 = time.perf_counter(); enc_out, enc_len = run_encoder(feats, int(flen[0])); t_enc = time.perf_counter() - t0
    t0 = time.perf_counter(); ids, frames, steps = tdt_greedy(enc_out, enc_len); t_dec = time.perf_counter() - t0
    hyp = detok(ids)
    ref = fx['transcript']
    e = wer(norm(ref), norm(hyp)); nw = len(norm(ref).split())
    tot_err += e * nw; tot_words += nw
    per_lang.setdefault(fx['language'], []).append(e)
    total = t_feat + t_enc + t_dec
    log(f"\n[{fx['language']}] {fx['file']} {fx['duration_s']}s  feat {t_feat*1000:.0f}ms enc {t_enc*1000:.0f}ms dec {t_dec*1000:.0f}ms ({steps} steps)  total {total*1000:.0f}ms RTF {total/fx['duration_s']:.3f}  WER {e:.2f}")
    log('   REF:', ref)
    log('   HYP:', hyp)
log('\n=== summary ===')
for l, es in per_lang.items(): log(f'  {l}: mean WER {np.mean(es):.3f} over {len(es)} clips')
log(f'  overall WER (word-weighted): {tot_err/max(1,tot_words):.3f}')
log('DONE')
