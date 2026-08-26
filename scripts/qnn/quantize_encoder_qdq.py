"""Quantize the static-shape Parakeet encoder to QDQ (uint8 weights / uint16 activations) for the QNN HTP backend.

Usage: python quantize_encoder_qdq.py --frames 1000 [--act u8|u16] [--calib-dir DIR] [--max-calib 16]
Input : work/encoder_static_b1_t{T}.onnx (created by probe_encoder_htp.py)
Output: work/encoder_qdq_{act}_t{T}.onnx (+ .data)

Follows the ORT QNN EP quantization recipe: qnn_preprocess_model() -> get_qnn_qdq_config() -> quantize().
Calibration: mel features (nemo128.onnx) of real speech clips, padded/trimmed to T frames.
"""
import argparse, glob, os, sys, time
import numpy as np
import onnx
import onnxruntime as ort
from onnxruntime.quantization import CalibrationDataReader, QuantType, CalibrationMethod, quantize
from onnxruntime.quantization.execution_providers.qnn import get_qnn_qdq_config, qnn_preprocess_model
import soundfile as sf

M = r'C:\Users\artam\AppData\Local\Temp\claude\C--Users-artam-OneDrive-Desktop-LocalWisper\99cce259-4884-4ea7-a3a6-ddeeafec6e91\scratchpad\models'
PREPROC = os.path.join(M, 'istupakov', 'nemo128.onnx')
WORK = r'C:\Users\artam\lwdev\work'

ap = argparse.ArgumentParser()
ap.add_argument('--frames', type=int, default=1000)
ap.add_argument('--act', default='u16', choices=['u8', 'u16'])
ap.add_argument('--calib-dir', default=r'C:\Users\artam\lwdev\fleurs')
ap.add_argument('--max-calib', type=int, default=16)
ap.add_argument('--method', default='minmax', choices=['minmax', 'percentile', 'entropy'])
args = ap.parse_args()
T = args.frames

src = os.path.join(WORK, f'encoder_static_b1_t{T}.onnx')
pre_path = os.path.join(WORK, f'encoder_static_b1_t{T}.qnnprep.onnx')
out_path = os.path.join(WORK, f'encoder_qdq_{args.act}_t{T}.onnx')

def log(*a):
    print(*a, flush=True)

def load_wav_16k(path):
    wav, sr = sf.read(path, dtype='float32')
    if wav.ndim > 1: wav = wav.mean(axis=1)
    if sr != 16000:
        n_out = int(round(len(wav) * 16000 / sr))
        wav = np.interp(np.linspace(0, len(wav) - 1, n_out), np.arange(len(wav)), wav).astype(np.float32)
    return wav

class MelReader(CalibrationDataReader):
    def __init__(self, wavs):
        self.pre = ort.InferenceSession(PREPROC, providers=['CPUExecutionProvider'])
        self.wavs = list(wavs); self.i = 0
    def get_next(self):
        if self.i >= len(self.wavs): return None
        w = load_wav_16k(self.wavs[self.i]); self.i += 1
        feats, flen = self.pre.run(None, {'waveforms': w[None, :], 'waveforms_lens': np.array([len(w)], dtype=np.int64)})
        x = np.zeros((1, 128, T), dtype=np.float32); n = min(T, feats.shape[2]); x[:, :, :n] = feats[:, :, :n]
        return {'audio_signal': x, 'length': np.array([n], dtype=np.int64)}
    def rewind(self): self.i = 0

wavs = sorted(glob.glob(os.path.join(args.calib_dir, '**', '*.wav'), recursive=True))
if not wavs:
    wavs = sorted(glob.glob(os.path.join(M, 'sherpa', '**', '*.wav'), recursive=True))
# spread across languages: take every k-th file
k = max(1, len(wavs) // args.max_calib)
wavs = wavs[::k][:args.max_calib]
log(f'calibration clips: {len(wavs)}'); [log('   ', os.path.basename(w)) for w in wavs]

t0 = time.perf_counter()
if not os.path.exists(pre_path):
    log('[1] qnn_preprocess_model ...')
    changed = qnn_preprocess_model(src, pre_path, fuse_layernorm=True, save_as_external_data=True, all_tensors_to_one_file=True,
                                   external_data_location=os.path.basename(pre_path) + '.data', external_data_size_threshold=1024)
    log(f'    modified={changed} in {time.perf_counter()-t0:.1f}s')
else:
    log('[1] reusing', pre_path)

act = QuantType.QUInt16 if args.act == 'u16' else QuantType.QUInt8
method = {'minmax': CalibrationMethod.MinMax, 'percentile': CalibrationMethod.Percentile, 'entropy': CalibrationMethod.Entropy}[args.method]
log(f'[2] get_qnn_qdq_config act={act} weight=QUInt8 per_channel=True method={method}')
model = onnx.load(pre_path, load_external_data=False)
cfg = get_qnn_qdq_config(pre_path, MelReader(wavs), calibrate_method=method, activation_type=act, weight_type=QuantType.QUInt8,
                         per_channel=True, keep_removable_activations=True, stride=None if T <= 1000 else None)
# keep the int64 'length' path and small index ops in fp/int: op types not to quantize
cfg.op_types_to_quantize = None
cfg.use_external_data_format = True
t0 = time.perf_counter()
log('[3] quantize() (calibration + QDQ insertion) ...')
quantize(pre_path, out_path, cfg)
log(f'    done in {time.perf_counter()-t0:.1f}s -> {out_path}')
m = onnx.load(out_path, load_external_data=False)
import collections
log('    ops:', dict(collections.Counter(n.op_type for n in m.graph.node).most_common(25)))
sz = sum(os.path.getsize(f) for f in glob.glob(out_path + '*'))
log(f'    size on disk: {sz/1e6:.1f} MB')
log('DONE')
