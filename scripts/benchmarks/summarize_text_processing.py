"""Archive local text research evidence and rebuild its machine-readable summary.

Run after the experiments; no models or runtime binaries are copied into docs.
"""
from __future__ import annotations

import argparse
import collections
import html
import json
from pathlib import Path
import shutil
import statistics

import text_processing_research as research

DESTINATION = research.ROOT / "docs/measurements/text-processing-2026-11-15"
EXCLUDED = {
    "smoke-qwen35.jsonl": "Initial smoke check; not a final timing run.",
    "marian-init-cpu.jsonl": "Startup probe overlapped the first ASR CPU experiment; replaced by marian-final-cpu.",
    "punct-init-cpu.jsonl": "Part of the overlapping startup-probe batch; replaced by punct-final-cpu.",
    "transcrib-06b-cuda.jsonl": "Published tokenizer omitted a template; fallback ChatML leaked empty thinking tags. Replaced by template runs.",
    "transcrib-06b-cpu.jsonl": "Missing original Qwen3 non-thinking template; replaced by template runs.",
    "transcrib-06b-holdout-cpu.jsonl": "Missing original Qwen3 non-thinking template; replaced by template runs.",
    "qwen3-06b-runtime-check-cuda.jsonl": "Backend verification only; no warm timing repetitions.",
}
REVIEW_RUNS = [
    "qwen3-06b-cuda", "qwen35-08b-cuda", "qwen3-17b-cuda",
    "hymt2-18b-final-cuda", "hymt2-18b-holdout-cuda",
    "qwen35-2b-conservative-cases-cuda", "qwen35-2b-conservative-holdout-cuda",
    "qwen35-4b-conservative-cases-cuda", "qwen35-4b-conservative-holdout-cuda",
    "qwen35-4b-final-cuda", "transcrib-06b-template-cuda",
    "transcrib-06b-template-holdout-cpu", "marian-final-cpu", "marian-beam4-cpu", "punct-final-cpu",
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=research.WORK)
    parser.add_argument("--destination", type=Path, default=DESTINATION)
    args = parser.parse_args()
    destination = args.destination
    raw = destination / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((args.source / "manifest.json").read_text("utf-8"))
    # Normalize GitHub's API metadata URLs to the original binary download URLs.
    for asset in manifest["runtime_assets"]:
        asset["url"] = f"https://github.com/ggml-org/llama.cpp/releases/download/{manifest['llama_tag']}/{asset['name']}"
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2), "utf-8")
    for name in ["baseline-manifest.json", "transcrib-conversion.json", "environment.json", "transcrib-chat-template.jinja", "asr-final-cpu.txt"]:
        shutil.copy2(args.source / name, destination / name)
    runs = []
    review_rows = collections.defaultdict(list)
    totals = collections.Counter()
    for filename in sorted(args.source.glob("*.jsonl")):
        shutil.copy2(filename, raw / filename.name)
        rows = [json.loads(line) for line in filename.read_text("utf-8").splitlines()]
        metadata = next(r for r in rows if r["type"] == "metadata")
        eligible = filename.name not in EXCLUDED
        first = [r for r in rows if r["type"] == "result" and r["repeat"] == "first"]
        warm = [r for r in rows if r["type"] == "result" and isinstance(r["repeat"], int)]
        groups = collections.defaultdict(list)
        for row in warm:
            groups[(row["task"], row["bucket"])].append(row["wall_ms"])
            if filename.stem in REVIEW_RUNS and row["repeat"] == 0:
                review_rows[row["case_id"]].append((filename.stem, row["output"]))
        run = {"file": f"raw/{filename.name}", "model": metadata["model"], "backend": metadata["backend"],
               "prompt_style": metadata.get("prompt_style", "publisher baseline"),
               "eligible": eligible, "exclusion_reason": EXCLUDED.get(filename.name),
               "load_ready_ms": metadata["load_ready_ms"], "import_ms": metadata.get("import_ms"),
               "first_request_ms": first[0]["wall_ms"] if first else None,
               "warm_requests": len(warm), "errors": sum(r["type"] == "error" for r in rows),
               "groups": [{"task": t, "bucket": b, "n": len(v), "median_ms": statistics.median(v),
                           "min_ms": min(v), "max_ms": max(v)} for (t, b), v in sorted(groups.items())],
               "offload_evidence": [r for r in rows if r["type"] == "runtime_verification"]}
        runs.append(run)
        if eligible:
            totals["warm_requests"] += len(warm)
            totals["first_requests"] += len(first)
            totals["errors"] += run["errors"]
            totals["cached_prompts"] += sum(r.get("timings", {}).get("cache_n", 0) != 0 for r in warm)
            totals["empty_outputs"] += sum(not r["output"].strip() for r in warm)
            totals["output_limit_hits"] += sum(r.get("stop_type") == "limit" or r.get("tokens_predicted", 0) >= 512 for r in warm)
    fixture_paths = [research.ROOT / "tests/fixtures/text-processing" / f for f in ["cases.json", "holdout.json"]]
    fixtures = [c for p in fixture_paths for c in json.loads(p.read_text("utf-8"))["cases"]]
    summary = {"date": "2026-11-15", "distinct_fixtures": len(fixtures),
               "fixture_tasks": dict(collections.Counter(c["task"] for c in fixtures)),
               "totals_for_eligible_runs": dict(totals), "runs": runs,
               "source_sha256": {str(p.relative_to(research.ROOT)): research.sha256(p) for p in fixture_paths +
                   [Path(__file__), Path(research.__file__), research.ROOT / "scripts/benchmarks/text_processing_baselines.py",
                    research.ROOT / "scripts/benchmarks/convert_transcrib_research.py",
                    research.ROOT / "scripts/benchmarks/text-processing-requirements.txt"]}}
    (destination / "summary.json").write_text(json.dumps(summary, indent=2), "utf-8")
    review = ["# Recorded outputs for human review", "",
              "References are examples, not unique correct answers. Conservative prompts intentionally do not resolve self-corrections.",
              "Protected-string checks in JSONL are diagnostics, not semantic scores. Date localization can trigger them without changing meaning.", ""]
    for case in fixtures:
        review += [f"## {case['id']}", "", f"Task: {case['task']}; language: {case['lang']}; target: {case.get('target', 'same language')}.", "",
                   f"Input: {case['input']}", "", f"Example reference: {case['reference']}", "",
                   "| Run | Recorded output, repeat 0 |", "|---|---|"]
        for run_name, output in review_rows[case["id"]]:
            rendered = html.escape(output).replace("|", "&#124;").replace("\n", "<br>")
            review.append(f"| `{run_name}` | {rendered} |")
        review.append("")
    (destination / "outputs.md").write_text("\n".join(review), "utf-8")
    print(json.dumps({"destination": str(destination), "fixtures": len(fixtures), "runs": len(runs), "totals": dict(totals)}, indent=2))


if __name__ == "__main__":
    main()
