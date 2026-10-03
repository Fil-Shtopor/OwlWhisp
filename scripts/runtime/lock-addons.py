"""Refresh the reviewed runtime add-on lock from pinned vendor package metadata."""
import base64
import concurrent.futures
import hashlib
import json
from pathlib import Path
import urllib.request
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
packages = []

def nuget(key, package, version, files):
    url = f"https://www.nuget.org/api/v2/Packages(Id='{package}',Version='{version}')"
    xml = ET.fromstring(urllib.request.urlopen(url, timeout=30).read())
    fields = {n.tag.split('}')[-1]: n.text for n in xml.iter()}
    assert fields['PackageHashAlgorithm'] == 'SHA512'
    return dict(id=key, url=f'https://api.nuget.org/v3-flatcontainer/{package}/{version}/{package}.{version}.nupkg',
                sha512=base64.b64decode(fields['PackageHash']).hex(), size=int(fields['PackageSize']), files=files)

def native(names, platform='win-x64'):
    return {f'runtimes/{platform}/native/{n}': n for n in names}

jobs = [
    ('cuda-core', 'microsoft.ml.onnxruntime.gpu.windows', '1.28.0',
     native(['onnxruntime.dll', 'onnxruntime_providers_shared.dll', 'onnxruntime_providers_cuda.dll', 'onnxruntime_providers_tensorrt.dll']) | {'LICENSE':'onnxruntime-gpu-LICENSE.txt', 'ThirdPartyNotices.txt':'onnxruntime-gpu-ThirdPartyNotices.txt'}),
    ('directml', 'microsoft.windows.ai.machinelearning', '2.5.77-rc',
     native(['onnxruntime.dll', 'DirectML.dll', 'Microsoft.Windows.AI.MachineLearning.dll']) | {'license.txt':'windows-ml-license.txt'}),
    ('webgpu', 'microsoft.ml.onnxruntime.ep.webgpu', '0.3.0',
     native(['onnxruntime_providers_webgpu.dll', 'dxcompiler.dll', 'dxil.dll']) | {'LICENSE':'onnxruntime-webgpu-LICENSE.txt'}),
]
for component in ['nvinfer_10', 'nvinfer_plugin_10', 'nvonnxparser_10'] + [f'nvinfer_builder_resource_sm{sm}_10' for sm in [75,80,86,89,90,120]]:
    files = native([component + '.dll'])
    files['LICENSE.txt'] = 'ntvlibs-tensorrt-LICENSE.txt'
    jobs.append((component, f'ntvlibs.tensorrt.cuda13.{component}.runtime.win-x64', '10.14.1.48', files))
with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
    packages.extend(pool.map(lambda args: nuget(*args), jobs))

cuda = json.load(urllib.request.urlopen('https://developer.download.nvidia.com/compute/cuda/redist/redistrib_13.0.1.json'))
cudnn = json.load(urllib.request.urlopen('https://developer.download.nvidia.com/compute/cudnn/redist/redistrib_9.13.1.json'))
for key, manifest, component, dlls in [
    ('cudart',cuda,'cuda_cudart',['cudart64_13.dll']),
    ('cublas',cuda,'libcublas',['cublas64_13.dll','cublasLt64_13.dll']),
    ('cufft',cuda,'libcufft',['cufft64_12.dll']),
    ('curand',cuda,'libcurand',['curand64_10.dll']),
    ('cudnn',cudnn,'cudnn',['cudnn64_9.dll','cudnn_graph64_9.dll','cudnn_ops64_9.dll','cudnn_heuristic64_9.dll','cudnn_adv64_9.dll','cudnn_cnn64_9.dll','cudnn_engines_precompiled64_9.dll','cudnn_engines_runtime_compiled64_9.dll']),
]:
    info = manifest[component]['windows-x86_64']
    if 'cuda13' in info: info = info['cuda13']
    relative = info['relative_path']
    base = 'cudnn' if key == 'cudnn' else 'cuda'
    folder = Path(relative).name.removesuffix('.zip')
    binary_dir = 'bin' if key == 'cudnn' else 'bin/x64'
    files = {f'{folder}/{binary_dir}/{dll}':dll for dll in dlls}
    files[f'{folder}/LICENSE'] = f'nvidia-{component}-LICENSE'
    packages.append(dict(id=key,url=f'https://developer.download.nvidia.com/compute/{base}/redist/{relative}',sha256=info['sha256'],size=int(info['size']),files=files))

# Pinned hashes also allow reuse of exactly matching bundled DLLs without downloading an archive.
for p in packages:
    p['bundled_sha256'] = {}
    source = ROOT / 'runtime' / ('win-x64-directml' if p['id']=='directml' else 'win-x64')
    for member, name in p['files'].items():
        file = source / name
        if file.is_file():
            p['bundled_sha256'][name] = hashlib.file_digest(file.open('rb'), 'sha256').hexdigest()
output = ROOT / 'crates/lw-app/src/runtime-packages.json'
output.write_text(json.dumps(packages, indent=2) + '\n', encoding='utf-8')
print(f'Locked {len(packages)} packages in {output}')
