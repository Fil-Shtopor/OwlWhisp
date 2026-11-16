"""Measure the two proposed text models together without PyTorch/Transformers.

Windows/NVIDIA research only. Reports process working sets and system-wide GPU
memory deltas, not a prediction of the final integrated Rust application's memory.
"""
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import time

import psutil

import text_processing_research as research


def gpu_used_mib():
    result = subprocess.run(["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
                            capture_output=True, text=True, check=True,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    return int(result.stdout.splitlines()[0])


def snapshot(pid):
    info = psutil.Process(pid).memory_info()
    return {"working_set_mib": info.rss / 2**20, "private_commit_mib": info.private / 2**20}


def main():
    import numpy as np
    import onnxruntime as ort
    from tokenizers import Tokenizer

    own_pid = psutil.Process().pid
    cases = json.loads((research.ROOT / "tests/fixtures/text-processing/cases.json").read_text("utf-8"))["cases"]
    result = {"scope": "Two isolated research workers loaded together, above existing desktop/ASR usage. Working sets may include shared pages; private commit is not physical RAM. GPU delta is system-wide, not process-attributed.",
              "punct_wrapper": "ONNX Runtime + native tokenizers; no Transformers/PyTorch",
              "timestamp_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "gpu_before_mib": gpu_used_mib(), "parent_before_models": snapshot(own_pid)}
    executable = next((research.WORK / "cuda").rglob("llama-server.exe"))
    command = [str(executable), "-m", str(research.WORK / "models/Hy-MT2-1.8B-Q4_K_M.gguf"),
               "--host", "127.0.0.1", "--port", "18765", "-c", "4096", "-np", "1",
               "-t", "8", "-tb", "8", "-ngl", "999", "--reasoning", "off", "--jinja",
               "--cache-ram", "0", "--no-ui", "-lv", "4"]
    result["translator_command"] = command
    with (research.WORK / "combined-memory-server.log").open("w", encoding="utf-8") as log:
        worker = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                  creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        try:
            started = time.perf_counter()
            while True:
                if worker.poll() is not None:
                    raise RuntimeError("Translator worker exited")
                try:
                    if research.get_json("http://127.0.0.1:18765/health").get("status") == "ok":
                        break
                except OSError:
                    pass
                if time.perf_counter() - started > 120:
                    raise TimeoutError("Translator startup timed out")
                time.sleep(0.1)
            folder = research.WORK / "punct-original"
            tokenizer = Tokenizer.from_file(str(folder / "tokenizer.json"))
            options = ort.SessionOptions()
            options.intra_op_num_threads = 8
            options.inter_op_num_threads = 1
            session = ort.InferenceSession(str(folder / "onnx/model_quantized.onnx"),
                                           sess_options=options, providers=["CPUExecutionProvider"])
            for case_id in ["c_ru_short", "c_ru_long"]:
                case = next(c for c in cases if c["id"] == case_id)
                encoding = tokenizer.encode(case["input"])
                ids = np.array([encoding.ids], dtype=np.int64)
                session.run(None, {"input_ids": ids, "attention_mask": np.ones_like(ids)})
            for case_id in ["t_ru_en_short", "t_ru_en_long"]:
                case = next(c for c in cases if c["id"] == case_id)
                prompt = research.post(18765, "/apply-template", {"messages": research.make_messages(case, "hymt2-18b")})
                research.post(18765, "/completion", {"prompt": prompt["prompt"], "n_predict": 512,
                                                     "temperature": 0, "cache_prompt": False})
            result.update(translator=snapshot(worker.pid), punctuation_worker=snapshot(own_pid),
                          gpu_loaded_mib=gpu_used_mib())
            result["sum_working_sets_mib"] = result["translator"]["working_set_mib"] + result["punctuation_worker"]["working_set_mib"]
            result["gpu_delta_mib"] = result["gpu_loaded_mib"] - result["gpu_before_mib"]
        finally:
            worker.terminate()
            try:
                worker.wait(timeout=10)
            except subprocess.TimeoutExpired:
                worker.kill()
                worker.wait()
    result["gpu_after_worker_exit_mib"] = gpu_used_mib()
    output = research.WORK / "combined-text-memory.json"
    output.write_text(json.dumps(result, indent=2), "utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
