import os, time, numpy as np, onnxruntime as ort, soundfile as sf, json
M = r'C:\Users\artam\AppData\Local\Temp\claude\C--Users-artam-OneDrive-Desktop-OwlWhisp\99cce259-4884-4ea7-a3a6-ddeeafec6e91\scratchpad\models'
SH = os.path.join(M,'sherpa','sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8')
pre = ort.InferenceSession(os.path.join(M,'istupakov','nemo128.onnx'), providers=['CPUExecutionProvider'])
wav, sr = sf.read(os.path.join(SH,'test_wavs','en.wav'), dtype='float32')
feats, flen = pre.run(None, {'waveforms': wav[None,:], 'waveforms_lens': np.array([len(wav)],dtype=np.int64)})
print('audio %.2fs feats %s' % (len(wav)/sr, feats.shape))
def bench(path, label, threads=None):
    so = ort.SessionOptions()
    if threads: so.intra_op_num_threads = threads
    t0=time.perf_counter(); s = ort.InferenceSession(path, so, providers=['CPUExecutionProvider']); tl=time.perf_counter()-t0
    for T in (500, 1000, 2000):
        x = np.zeros((1,128,T), dtype=np.float32); n=min(T, feats.shape[2]); x[:,:,:n]=feats[:,:,:n]
        L = np.array([T], dtype=np.int64)
        s.run(None, {'audio_signal': x, 'length': L})
        ts=[]
        for _ in range(3):
            t0=time.perf_counter(); out=s.run(None, {'audio_signal': x, 'length': L}); ts.append(time.perf_counter()-t0)
        print(f'{label:28} threads={threads or "default":>7} T={T:5} ({T/100:4.0f}s audio): {min(ts)*1000:7.0f} ms  RTF={min(ts)/(T/100):.3f}   (load {tl:.1f}s)')
bench(os.path.join(SH,'encoder.int8.onnx'), 'sherpa encoder int8')
bench(os.path.join(SH,'encoder.int8.onnx'), 'sherpa encoder int8', 8)
bench(os.path.join(SH,'encoder.int8.onnx'), 'sherpa encoder int8', 4)
bench(os.path.join(M,'istupakov','encoder-model.onnx'), 'istupakov encoder fp32')
