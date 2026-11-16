"""CPU Marian INT8 and punctuation/truecase ONNX baselines for OwlWhisp research.

Run in the isolated research venv. No application integration or cloud inference.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import random
import re
import time

# The punctuation decoder follows truecase.py by Horizon Labs, Apache-2.0.
# Copyright 2026 Horizon Labs. Source (pinned below):
# https://huggingface.co/Horizon-Labs/multilingual-punctuation-truecase-small
import text_processing_research as research

WORK = research.WORK
ROOT = research.ROOT
MARIAN_REPO = "Helsinki-NLP/opus-mt-ru-en"
MARIAN_REVISION = "fbd6dc73284f95536648512cc21d57f19191961a"
PUNCT_REPO = "Horizon-Labs/multilingual-punctuation-truecase-small"
PUNCT_REVISION = "a527b62e5a3e16de7e316f03234c99422b1c8814"


def fetch(repo, revision, filenames, folder):
    with research.concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        futures = [pool.submit(research.download,
                   f"https://huggingface.co/{repo}/resolve/{revision}/{filename}", folder / filename)
                   for filename in filenames]
        for future in futures:
            future.result()
    return {"repo": repo, "revision": revision,
            "files": {f: {"sha256": research.sha256(folder / f), "bytes": (folder / f).stat().st_size}
                      for f in filenames}}


def prepare():
    marian = fetch(MARIAN_REPO, MARIAN_REVISION,
                   ["README.md", "config.json", "generation_config.json", "tokenizer_config.json",
                    "source.spm", "target.spm", "vocab.json", "pytorch_model.bin"], WORK / "marian-original")
    punct = fetch(PUNCT_REPO, PUNCT_REVISION,
                  ["README.md", "LICENSE", "NOTICE", "config.json", "tokenizer.json", "tokenizer_config.json",
                   "special_tokens_map.json", "onnx/model_quantized.onnx"], WORK / "punct-original")
    (WORK / "baseline-manifest.json").write_text(json.dumps({"marian": marian, "punct": punct}, indent=2), "utf-8")
    import ctranslate2
    target = WORK / "marian-ct2-int8"
    if not (target / "model.bin").exists():
        converter = ctranslate2.converters.TransformersConverter(str(WORK / "marian-original"))
        converter.convert(str(target), quantization="int8")
    print("Baselines ready", flush=True)


def marian(threads, beam_size):
    import ctranslate2
    from transformers import MarianTokenizer
    tokenizer = MarianTokenizer.from_pretrained(str(WORK / "marian-original"), local_files_only=True)
    translator = ctranslate2.Translator(str(WORK / "marian-ct2-int8"), device="cpu", compute_type="int8",
                                        inter_threads=1, intra_threads=threads)

    def translate(text):
        tokens = tokenizer.convert_ids_to_tokens(tokenizer.encode(text))
        result = translator.translate_batch([tokens], beam_size=beam_size, max_decoding_length=512)
        return tokenizer.decode(tokenizer.convert_tokens_to_ids(result[0].hypotheses[0]), skip_special_tokens=True)

    return translate


def punct(threads):
    """Publisher's word-end decoding for the RU/EN/ES fixture languages, using ONNX INT8.

    Based on the inspected Apache-2.0 truecase.py in the pinned model repository.
    No remotely downloaded Python is executed. CJK/script-specific marks are outside this suite.
    """
    import numpy as np
    import onnxruntime as ort
    from transformers import AutoTokenizer
    folder = WORK / "punct-original"
    tokenizer = AutoTokenizer.from_pretrained(str(folder), local_files_only=True)
    config = json.loads((folder / "config.json").read_text("utf-8"))
    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    options.inter_op_num_threads = 1
    session = ort.InferenceSession(str(folder / "onnx/model_quantized.onnx"), sess_options=options,
                                   providers=["CPUExecutionProvider"])
    input_names = [x.name for x in session.get_inputs()]
    print("ONNX inputs:", input_names, flush=True)
    marks = re.compile(r"[.,;:!?。，、；：！？؟،¿¡…।॥۔።፣፧]")

    def restore(text):
        # Match the publisher's numeric-punctuation exception and same-length lowercasing.
        text = marks.sub(lambda m: m.group() if m.group() in ".,:" and 0 < m.start() < len(text) - 1
                         and text[m.start()-1].isdigit() and text[m.start()+1].isdigit() else "", text)
        text = "".join(c.lower() if len(c.lower()) == 1 else c for c in re.sub(r"\s+", " ", text).strip())
        encoding = tokenizer(text, return_offsets_mapping=True, add_special_tokens=False)
        ids, offsets = encoding["input_ids"], encoding["offset_mapping"]
        labels = {}
        start = 0
        while start < len(ids):
            end = min(len(ids), start + 400)
            input_ids = np.array([[tokenizer.cls_token_id] + ids[start:end] + [tokenizer.sep_token_id]], dtype=np.int64)
            feeds = {"input_ids": input_ids}
            if "attention_mask" in input_names:
                feeds["attention_mask"] = np.ones_like(input_ids)
            if "token_type_ids" in input_names:
                feeds["token_type_ids"] = np.zeros_like(input_ids)
            predicted = session.run(None, feeds)[0][0, 1:-1].argmax(-1)
            keep_to = end if end == len(ids) else end - 50
            for index in range(start, keep_to):
                s, e = offsets[index]
                if e > s:
                    labels[e] = config["id2label"][str(predicted[index-start])]
            if end == len(ids):
                break
            start = keep_to
        result, previous = [], 0
        for end in sorted(labels):
            if not (end == len(text) or text[end] == " "):
                continue
            mark, casing = labels[end].split("|")
            start = max(previous, text.rfind(" ", 0, end) + 1)
            word = text[start:end]
            if casing == "UPPER" and len(word.upper()) == len(word):
                word = word.upper()
            elif casing == "Cap":
                for index, char in enumerate(word):
                    if char.lower() != char.upper():
                        word = word[:index] + char.upper() + word[index+1:]
                        break
            result.append(text[previous:start] + word + ("" if mark == "0" else " -" if mark == "-" else mark))
            previous = end
        return "".join(result) + text[previous:]

    return restore


def run(args):
    import importlib.metadata
    import psutil
    os.environ["TOKENIZERS_PARALLELISM"] = "false"
    cases = json.loads((ROOT / "tests/fixtures/text-processing/cases.json").read_text("utf-8"))["cases"]
    if args.model == "marian":
        cases = [c for c in cases if c["lang"] == "ru" and c["task"] in ["translation", "combined"] and c.get("target") == "English"]
        factory = lambda: marian(args.threads, args.beam_size)
    else:
        cases = [c for c in cases if c["task"] == "cleanup"]
        factory = lambda: punct(args.threads)
    imports_started = time.perf_counter()
    if args.model == "marian":
        import ctranslate2
        from transformers import MarianTokenizer
    else:
        import numpy
        import onnxruntime
        from transformers import AutoTokenizer
    import_ms = (time.perf_counter() - imports_started) * 1000
    started = time.perf_counter()
    process = factory()
    load_ms = (time.perf_counter() - started) * 1000
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", buffering=1) as out:
        out.write(json.dumps({"type": "metadata", "model": args.model, "backend": "cpu", "threads": args.threads,
                              "beam_size": args.beam_size if args.model == "marian" else None,
                              "load_ready_ms": load_ms, "import_ms": import_ms,
                              "versions": {p: importlib.metadata.version(p) for p in ["torch", "transformers", "ctranslate2", "onnxruntime", "sentencepiece"]}}) + "\n")
        schedule = [("first", cases[0])]
        rng = random.Random(15112026)
        for repeat in range(args.repeats):
            order = cases.copy()
            rng.shuffle(order)
            schedule.extend((repeat, c) for c in order)
        for repeat, case in schedule:
            started = time.perf_counter()
            text = process(case["input"])
            elapsed = (time.perf_counter() - started) * 1000
            row = {"type": "result", "model": args.model, "backend": "cpu", "case_id": case["id"],
                   "task": case["task"], "bucket": case["bucket"], "repeat": repeat,
                   "input": case["input"], "output": text, "wall_ms": elapsed,
                   "missing_protected": [p for p in case.get("protected", []) if p.casefold() not in text.casefold()],
                   "memory": research.memory_snapshot(psutil.Process().pid)}
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
            print(f"{repeat} {case['id']} {elapsed:.0f}ms", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "run"])
    parser.add_argument("--model", choices=["marian", "punct"])
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument("--beam-size", type=int, default=1)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--output")
    args = parser.parse_args()
    if args.command == "prepare":
        prepare()
    else:
        if not args.model or not args.output:
            parser.error("run requires --model and --output")
        run(args)


if __name__ == "__main__":
    main()
