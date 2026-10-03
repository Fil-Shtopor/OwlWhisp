"""Probe: can the Parakeet TDT 0.6B v3 encoder run on the Snapdragon X2 Elite HTP (V81) via ORT QNN EP?

Usage: python probe_encoder_htp.py --frames 1000 --mode fp16 [--ctx]
  --frames N     static mel-frame count (10 ms hop => N/100 seconds of audio)
  --mode fp16    run fp32 graph with enable_htp_fp16_precision=1 (no quantization)
  --mode qdq     run a QDQ-quantized graph produced by quantize_encoder_qdq.py (path via --model)
  --ctx          also dump an EPContext (QNN context binary) model and time reloading it
Writes a log to probe_<mode>_<frames>.log (flushed per line) and prints a summary.
"""
import argparse, os, sys, time, json, traceback
import numpy as np
import onnx
from onnx import shape_inference
import onnxruntime as ort
from onnxruntime.tools import onnx_model_utils

LIBDIR = r'C:\Users\artam\lwdev\venv\Lib\site-packages\onnxruntime_qnn\libs\arm64ec'
M = r'C:\Users\artam\AppData\Local\Temp\claude\C--Users-artam-OneDrive-Desktop-OwlWhisp\99cce259-4884-4ea7-a3a6-ddeeafec6e91\scratchpad\models'
ENC_FP32 = os.path.join(M, 'istupakov', 'encoder-model.onnx')
PREPROC = os.path.join(M, 'istupakov', 'nemo128.onnx')
WAV = os.path.join(M, 'sherpa', 'sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8', 'test_wavs', 'en.wav')
WORK = r'C:\Users\artam\lwdev\work'
os.makedirs(WORK, exist_ok=True)

ap = argparse.ArgumentParser()
ap.add_argument('--frames', type=int, default=1000)
ap.add_argument('--mode', default='fp16', choices=['fp16', 'fp32', 'qdq'])
ap.add_argument('--model', default=None, help='override input model (e.g. QDQ model)')
ap.add_argument('--ctx', action='store_true')
ap.add_argument('--skip-cpu', action='store_true')
args = ap.parse_args()

logf = open(os.path.join(WORK, f'probe_{args.mode}_{args.frames}.log'), 'w', encoding='utf-8')
def log(*a):
    s = ' '.join(str(x) for x in a)
    print(s, flush=True); logf.write(s + '\n'); logf.flush()

T = args.frames
static_path = args.model or os.path.join(WORK, f'encoder_static_b1_t{T}.onnx')

t0 = time.perf_counter()
if not os.path.exists(static_path):
    log(f'[1] loading fp32 encoder {ENC_FP32}')
    m = onnx.load(ENC_FP32)  # loads external data too
    log(f'    loaded in {time.perf_counter()-t0:.1f}s, nodes={len(m.graph.node)}')
    onnx_model_utils.make_dim_param_fixed(m.graph, 'audio_signal_dynamic_axes_1', 1)
    onnx_model_utils.make_dim_param_fixed(m.graph, 'audio_signal_dynamic_axes_2', T)
    onnx_model_utils.make_dim_param_fixed(m.graph, 'length_dynamic_axes_1', 1)
    # fix_output_shapes() would serialize the >2 GB proto in-memory and fail; ORT infers output shapes itself.
    tmp = os.path.join(WORK, f'encoder_fixed_b1_t{T}.onnx')
    onnx.save(m, tmp, save_as_external_data=True, all_tensors_to_one_file=True, location=os.path.basename(tmp) + '.data')
    del m
    log(f'[2] constant-folding with ORT BASIC optimizations -> {static_path}')
    so = ort.SessionOptions()
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
    so.optimized_model_filepath = static_path
    so.add_session_config_entry('session.optimized_model_external_initializers_file_name', os.path.basename(static_path) + '.data')
    so.add_session_config_entry('session.optimized_model_external_initializers_min_size_in_bytes', '1024')
    _ = ort.InferenceSession(tmp, so, providers=['CPUExecutionProvider'])
    del _
    ms = onnx.load(static_path, load_external_data=False)
    import collections
    log(f'    static model nodes={len(ms.graph.node)} ops={dict(collections.Counter(n.op_type for n in ms.graph.node).most_common(30))}')
    log(f'    done in {time.perf_counter()-t0:.1f}s')
else:
    log(f'[1-2] reusing static model {static_path}')

# ---- features from real audio (pad/trim to T frames)
import soundfile as sf
wav, sr = sf.read(WAV, dtype='float32')
if wav.ndim > 1: wav = wav.mean(axis=1)
if sr != 16000:  # probe-only linear resample; the app uses a proper polyphase resampler
    n_out = int(round(len(wav) * 16000 / sr))
    wav = np.interp(np.linspace(0, len(wav) - 1, n_out), np.arange(len(wav)), wav).astype(np.float32)
    sr = 16000
pre = ort.InferenceSession(PREPROC, providers=['CPUExecutionProvider'])
feats, flen = pre.run(None, {'waveforms': wav[None, :], 'waveforms_lens': np.array([len(wav)], dtype=np.int64)})
log(f'[3] features from {os.path.basename(WAV)}: {feats.shape} (len {int(flen[0])}), audio {len(wav)/sr:.2f}s')
if feats.shape[2] >= T:
    x = feats[:, :, :T].astype(np.float32); L = np.array([T], dtype=np.int64)
else:
    x = np.zeros((1, 128, T), dtype=np.float32); x[:, :, :feats.shape[2]] = feats; L = np.array([int(flen[0])], dtype=np.int64)

# ---- CPU reference
ref = None
if not args.skip_cpu:
    t0 = time.perf_counter()
    cpu = ort.InferenceSession(static_path, providers=['CPUExecutionProvider'])
    log(f'[4] CPU session created in {time.perf_counter()-t0:.1f}s')
    cpu.run(None, {'audio_signal': x, 'length': L})
    ts = []
    for _ in range(3):
        t0 = time.perf_counter(); ref, ref_len = cpu.run(None, {'audio_signal': x, 'length': L}); ts.append(time.perf_counter() - t0)
    log(f'    CPU fp32 run: {min(ts)*1000:.0f} ms (best of 3), out {ref.shape} enc_len {int(ref_len[0])}  => RTF {min(ts)/(T/100):.3f}')
    del cpu

# ---- QNN HTP session
os.add_dll_directory(LIBDIR); os.environ['PATH'] = LIBDIR + os.pathsep + os.environ['PATH']
ort.register_execution_provider_library('QNNExecutionProvider', os.path.join(LIBDIR, 'onnxruntime_providers_qnn.dll'))
npu = [d for d in ort.get_ep_devices() if d.ep_name == 'QNNExecutionProvider' and d.device.type == ort.OrtHardwareDeviceType.NPU]
log(f'[5] NPU devices: {len(npu)}')
so = ort.SessionOptions()
so.log_severity_level = 1  # INFO: prints node assignment summary
so.log_verbosity_level = 0
so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
opts = {'backend_type': 'htp', 'htp_performance_mode': 'burst', 'htp_graph_finalization_optimization_mode': '3'}
if args.mode == 'fp16':
    opts['enable_htp_fp16_precision'] = '1'
if args.mode == 'qdq':
    opts['offload_graph_io_quantization'] = '1'
ctx_path = os.path.join(WORK, f'encoder_{args.mode}_t{T}_ctx.onnx')
if args.ctx:
    so.add_session_config_entry('ep.context_enable', '1')
    so.add_session_config_entry('ep.context_file_path', ctx_path)
    so.add_session_config_entry('ep.context_embed_mode', '0')
so.add_provider_for_devices(npu, opts)
log(f'    options: {opts}')
t0 = time.perf_counter()
try:
    sess = ort.InferenceSession(static_path, so)
except Exception as e:
    log(f'!!! QNN session creation FAILED after {time.perf_counter()-t0:.1f}s: {type(e).__name__}: {str(e)[:2000]}')
    sys.exit(2)
prep = time.perf_counter() - t0
log(f'[6] QNN session created (graph prepare) in {prep:.1f}s; providers={sess.get_providers()}')
try:
    t0 = time.perf_counter(); out, out_len = sess.run(None, {'audio_signal': x, 'length': L}); log(f'    first run {(time.perf_counter()-t0)*1000:.0f} ms')
    ts = []
    for _ in range(5):
        t0 = time.perf_counter(); out, out_len = sess.run(None, {'audio_signal': x, 'length': L}); ts.append(time.perf_counter() - t0)
    log(f'[7] HTP run: best {min(ts)*1000:.0f} ms, median {sorted(ts)[2]*1000:.0f} ms => RTF {min(ts)/(T/100):.4f}; out {out.shape} enc_len {int(out_len[0])}')
    if ref is not None:
        n = int(min(ref_len[0], out_len[0]))
        a = ref[0, :, :n].astype(np.float64); b = out[0, :, :n].astype(np.float64)
        cos = float((a * b).sum() / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-9))
        log(f'[8] vs CPU fp32: cosine={cos:.6f} max_abs_diff={float(np.abs(a-b).max()):.4f} mean_abs_diff={float(np.abs(a-b).mean()):.5f} ref_mean_abs={float(np.abs(a).mean()):.4f}')
        np.save(os.path.join(WORK, f'enc_out_htp_{args.mode}_t{T}.npy'), out); np.save(os.path.join(WORK, f'enc_out_cpu_t{T}.npy'), ref)
except Exception as e:
    log(f'!!! QNN run FAILED: {type(e).__name__}: {str(e)[:2000]}')
    sys.exit(3)
if args.ctx and os.path.exists(ctx_path):
    bins = [f for f in os.listdir(WORK) if f.startswith(os.path.basename(ctx_path).replace('.onnx', '')) ]
    log(f'[9] EPContext model written: {ctx_path} ; related files: {bins}')
    for f in bins:
        log(f'    {f}: {os.path.getsize(os.path.join(WORK, f))/1e6:.1f} MB')
    del sess
    so2 = ort.SessionOptions(); so2.add_provider_for_devices(npu, {'backend_type': 'htp', 'htp_performance_mode': 'burst'})
    t0 = time.perf_counter(); sess2 = ort.InferenceSession(ctx_path, so2); log(f'    reload from context binary: {time.perf_counter()-t0:.2f}s')
    t0 = time.perf_counter(); out2, _ = sess2.run(None, {'audio_signal': x, 'length': L}); log(f'    run from context: {(time.perf_counter()-t0)*1000:.0f} ms; identical={np.allclose(out, out2)}')
log('DONE')
