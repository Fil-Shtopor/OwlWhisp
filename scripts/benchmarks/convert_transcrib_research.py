"""Experimental Windows conversion of a published MLX-only Russian cleanup model.

This is NOT the author's original GGUF. MLX affine INT4 weights are dequantized
to BF16 using the documented packed-affine representation, then exported to Q8_0
by pinned llama.cpp tooling. No claim of parity with an Apple MLX run is made.
All inputs, intermediate weights, and outputs stay in the ignored experiment folder.
"""
from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile

import text_processing_research as research

REPO = "NicolaiMTLassen/transcrib-cleanup-0.6b"
REVISION = "881a17920f1a97e3adc155978188ad80c97bb0fb"
FILES = ["README.md", "config.json", "tokenizer.json", "tokenizer_config.json", "model.safetensors"]


def main():
    import torch
    from safetensors.torch import load_file, save_file

    torch.set_num_threads(8)
    original = research.WORK / "transcrib-mlx"
    converted = research.WORK / "transcrib-hf-bf16"
    converted.mkdir(parents=True, exist_ok=True)
    for filename in FILES:
        research.download(f"https://huggingface.co/{REPO}/resolve/{REVISION}/{filename}", original / filename)
    archive = research.WORK / "downloads/llama-source-b12420.tar.gz"
    research.download("https://codeload.github.com/ggml-org/llama.cpp/tar.gz/refs/tags/b12420", archive)
    source = research.WORK / "source/llama.cpp-b12420"
    if not (source / "convert_hf_to_gguf.py").exists():
        with tarfile.open(archive) as tar:
            tar.extractall(research.WORK / "source", filter="data")
    if not (converted / "model.safetensors").exists():
        config = json.loads((original / "config.json").read_text("utf-8"))
        group_size = config["quantization"]["group_size"]
        assert config["quantization"]["bits"] == 4 and config["quantization"]["mode"] == "affine"
        config.pop("quantization")
        config.pop("quantization_config")
        (converted / "config.json").write_text(json.dumps(config, indent=2), "utf-8")
        for filename in ["tokenizer.json", "tokenizer_config.json"]:
            shutil.copy2(original / filename, converted / filename)
        weights = load_file(str(original / "model.safetensors"))
        output = {}
        shifts = torch.arange(0, 32, 4, dtype=torch.int64)
        # Each uint32 packs 8 consecutive 4-bit values, low bits first.
        probe = torch.tensor([0x76543210], dtype=torch.int64)
        assert torch.equal(((probe[:, None] >> shifts) & 15).reshape(-1), torch.arange(8))
        for name, tensor in weights.items():
            if name.endswith((".scales", ".biases")):
                continue
            prefix = name.removesuffix(".weight")
            if prefix + ".scales" in weights:
                scales = weights[prefix + ".scales"].float()
                biases = weights[prefix + ".biases"].float()
                n_rows = tensor.shape[0]
                unpacked = ((tensor.to(torch.int64).unsqueeze(-1) >> shifts) & 15).reshape(n_rows, -1, group_size).float()
                restored = (unpacked * scales.unsqueeze(-1) + biases.unsqueeze(-1)).reshape(n_rows, -1)
                assert torch.isfinite(restored).all()
                output[name] = restored.to(torch.bfloat16).contiguous()
            else:
                output[name] = tensor.to(torch.bfloat16).contiguous()
        print(f"Restored {sum(t.numel() for t in output.values()):,} parameters", flush=True)
        save_file(output, str(converted / "model.safetensors"), metadata={"format": "pt"})
    target = research.WORK / "models/Transcrib-Cleanup-0.6B-Q8_0.gguf"
    if not target.exists():
        subprocess.run([sys.executable, str(source / "convert_hf_to_gguf.py"), str(converted),
                        "--outfile", str(target), "--outtype", "q8_0"], check=True)
    manifest = {"source_repo": REPO, "source_revision": REVISION,
                "source_files": {f: research.sha256(original / f) for f in FILES},
                "converter_tag": research.LLAMA_TAG, "converter_archive_sha256": research.sha256(archive),
                "conversion": "MLX affine INT4 group64 -> BF16 -> GGUF Q8_0; experimental, no Apple MLX parity test",
                "output_file": target.name, "output_bytes": target.stat().st_size,
                "output_sha256": research.sha256(target)}
    (research.WORK / "transcrib-conversion.json").write_text(json.dumps(manifest, indent=2), "utf-8")
    print(json.dumps(manifest, indent=2), flush=True)


if __name__ == "__main__":
    main()
