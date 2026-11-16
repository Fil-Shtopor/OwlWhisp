"""Render auditable before/after examples from tested fixtures and native model output."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/text-processing"
EVIDENCE = ROOT / "docs/measurements/text-processing-2026-11-15"


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def cell(value):
    return str(value).replace("|", "\\|").replace("\n", "<br>") or "(пустой текст)"


def table(headers, rows):
    return "\n".join([
        "| " + " | ".join(headers) + " |",
        "|" + "---|" * len(headers),
        *("| " + " | ".join(cell(v) for v in row) + " |" for row in rows),
    ])


def main():
    fillers = read(FIXTURES / "multilingual-cleanup.json")
    punctuation = read(FIXTURES / "punctuation.json")
    cpu = read(EVIDENCE / "implementation-cpu.json")
    cuda = read(EVIDENCE / "implementation-cuda.json")
    cpu_rows = {(r["target"], r["input"]): r for r in cpu["results"]}
    chosen, seen = [], set()
    for case in fillers:
        if case["input"] != case["expected"] and case["language"] not in seen:
            if case["language"] == "auto":
                continue
            chosen.append(case)
            seen.add(case["language"])
    unchanged, seen = [], set()
    for case in fillers:
        if case["language"] in ["english", "russian", "german", "portuguese", "chinese"] and case["input"] == case["expected"] and case["language"] not in seen:
            unchanged.append(case)
            seen.add(case["language"])
    assert len(chosen) == 17
    assert all(cpu_rows[(r["target"], r["input"])]["output"] == r["output"] for r in cuda["results"])
    parts = [
        "# OwlWhisp 0.2.1: примеры до и после",
        "Это тексты на входе и выходе этапов обработки **после распознавания речи**. "
        "Они не являются расшифровкой новой микрофонной записи. Для правил показаны "
        "ожидаемые результаты, проверяемые Rust-тестами; для перевода — сохранённые "
        "фактические ответы локальной модели. Замеры перевода не включают время ASR.",
        "## 1. Только удаление паразитов",
        "Включён только **Remove filler sounds**; язык задан явно. Регистр здесь не исправляется. "
        "Поэтому после удаления начального междометия слово может остаться со строчной буквы. "
        "При желании включите также **Punctuation and capitalization**.",
        table(["Язык", "До", "После"], [(r["language"], r["input"], r["expected"]) for r in chosen]),
        "### Что намеренно остаётся",
        "Слова с возможным полезным смыслом не удаляются. Это ограничение консервативного режима.",
        table(["Язык", "До", "После"], [(r["language"], r["input"], r["expected"]) for r in unchanged]),
        "Источник: [полный набор, включая код, цитаты, пустой результат и автоопределение языка]"
        "(../tests/fixtures/text-processing/multilingual-cleanup.json). "
        "Проверка: `text::fillers::tests::multilingual_outcome_fixture_and_idempotence`.",
        "## 2. Только пунктуация и регистр",
        "Включена только **Punctuation and capitalization**. Точки, запятые и вопросительные "
        "знаки в этих примерах уже присутствуют на входе. Правила убирают лишние пробелы и "
        "исправляют начало предложения; отсутствующую пунктуацию они не угадывают.",
        table(["Язык", "До", "После"], [(r["language"], r["input"], r["expected"]) for r in punctuation]),
        "Пример `where is the report` показывает ограничение: появилась заглавная буква, "
        "но вопросительный знак не добавился. `iPhone` и `eBay` сохраняют исходный регистр. "
        "Французские пробелы перед `!` и `?` сохраняются.",
        "Источник: [проверяемые примеры](../tests/fixtures/text-processing/punctuation.json). "
        "Проверка: `text::punctuation::tests::published_examples_match_the_processor_and_are_idempotent`.",
        "## 3. Только перевод",
        "Ниже фактический результат **Hy-MT2-1.8B Q4_K_M** на CPU и CUDA. В этой серии "
        "ответы совпали; времена округлены до миллисекунд. Модель уже была загружена. "
        "Это демонстрационная выборка, а не доказательство качества на всём языке.",
        table(["Перевести на", "До", "После", "CPU, мс", "CUDA, мс"], [
            (r["target"], r["input"], r["output"], round(cpu_rows[(r["target"],r["input"])]["ms"]), round(r["ms"]))
            for r in cuda["results"]
        ]),
        "В техническом примере сохранились `debug_mode=true`, `settings.toml` и `/api/v2/users`. "
        "Цитируемая проза переводится: «привет» становится “hello”. Формулировки, вежливость "
        "и финальная пунктуация могут изменяться: например, китайский ответ использует «提交» "
        "(представить/подать) для английского `send`. Когда точная формулировка важна, проверьте "
        "перевод и воспользуйтесь **Copy original transcript**.",
        "Источники: [CPU](measurements/text-processing-2026-11-15/implementation-cpu.json), "
        "[CUDA](measurements/text-processing-2026-11-15/implementation-cuda.json), "
        "[нативный эксперимент](../crates/lw-app/examples/text_processing_smoke.rs).",
        "## 4. Две очистки вместе",
        table(["До", "После"], [("Um, please send the report uh, by Friday . thank you !", "Please send the report by Friday. Thank you!")]),
        "Этот результат также проверяется `assert_eq!` в нативном эксперименте. Перевод в этом "
        "примере не включён. Если включить все три режима, сначала выполняются очистка и "
        "пользовательский словарь, затем перевод.",
        "## Как повторить",
        "```powershell\n"
        "cargo test --locked -p lw-core text::\n"
        "cargo run --locked -p lw-app --example text_processing_smoke -- .owlwhisp-test/text-feature-preview/settings.json cpu\n"
        "# Вместо cpu можно указать cuda. При отсутствии файлов пример скачает переводчик.\n"
        "python scripts/benchmarks/render_text_processing_examples.py\n```",
        "Все три опции по умолчанию выключены. Для перевода нужен отдельный скачиваемый "
        "локальный пакет. При ошибке или таймауте вставляется очищенный исходный текст; "
        "уведомление в Dictate объясняет причину. "
        "[Полный исследовательский отчёт](text-processing-research-2026-11-15.md).",
    ]
    destination = ROOT / "docs/text-processing-examples-v0.2.1.md"
    destination.write_text("\n\n".join(parts) + "\n", encoding="utf-8")
    print(f"Rendered {len(chosen)} filler, {len(punctuation)} punctuation and {len(cuda['results'])} translation examples")


if __name__ == "__main__":
    main()
