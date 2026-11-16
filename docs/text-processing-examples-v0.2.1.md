# OwlWhisp 0.2.1: примеры до и после

Это тексты на входе и выходе этапов обработки **после распознавания речи**. Они не являются расшифровкой новой микрофонной записи. Для правил показаны ожидаемые результаты, проверяемые Rust-тестами; для перевода — сохранённые фактические ответы локальной модели. Замеры перевода не включают время ASR.

## 1. Только удаление паразитов

Включён только **Remove filler sounds**; язык задан явно. Регистр здесь не исправляется. Поэтому после удаления начального междометия слово может остаться со строчной буквы. При желании включите также **Punctuation and capitalization**.

| Язык | До | После |
|---|---|---|
| english | Um, I like this, uh, really. | I like this, really. |
| russian | Эм, привет, ну давай завтра. | привет, ну давай завтра. |
| ukrainian | Е-е, це мій звіт. | це мій звіт. |
| spanish | Eh, hola, necesito el informe. | hola, necesito el informe. |
| italian | Ehm, ciao, vorrei questo. | ciao, vorrei questo. |
| french | Euh, bonjour, je veux ceci. | bonjour, je veux ceci. |
| german | Ähm, ich komme um acht. | ich komme um acht. |
| portuguese | Ahn, quero ficar em casa. | quero ficar em casa. |
| dutch | Ehm, ik stuur het bestand. | ik stuur het bestand. |
| polish | Yyy, proszę wysłać plik. | proszę wysłać plik. |
| czech | Ehm, prosím pošli soubor. | prosím pošli soubor. |
| turkish | Iıı, merhaba. | merhaba. |
| chinese | 呃，我想明天开会。 | 我想明天开会。 |
| japanese | えーと、明日お願いします。 | 明日お願いします。 |
| korean | 음, 내일 만나요. | 내일 만나요. |
| arabic | أمم، مرحبا | مرحبا |
| hindi | उम्म, कल मिलते हैं। | कल मिलते हैं। |

### Что намеренно остаётся

Слова с возможным полезным смыслом не удаляются. Это ограничение консервативного режима.

| Язык | До | После |
|---|---|---|
| english | Well, I like the sound of that, you know. | Well, I like the sound of that, you know. |
| russian | Как бы ни менялся срок, это ошибка типа данных. | Как бы ни менялся срок, это ошибка типа данных. |
| german | Ich komme um acht, also morgen. | Ich komme um acht, also morgen. |
| portuguese | Um livro, tipo este, é bom. | Um livro, tipo este, é bom. |
| chinese | 嗯，我同意那个方案。呃逆是一种症状。 | 嗯，我同意那个方案。呃逆是一种症状。 |

Источник: [полный набор, включая код, цитаты, пустой результат и автоопределение языка](../tests/fixtures/text-processing/multilingual-cleanup.json). Проверка: `text::fillers::tests::multilingual_outcome_fixture_and_idempotence`.

## 2. Только пунктуация и регистр

Включена только **Punctuation and capitalization**. Точки, запятые и вопросительные знаки в этих примерах уже присутствуют на входе. Правила убирают лишние пробелы и исправляют начало предложения; отсутствующую пунктуацию они не угадывают.

| Язык | До | После |
|---|---|---|
| english | hello , world . how are you ? | Hello, world. How are you? |
| russian | привет , мир . как дела ? | Привет, мир. Как дела? |
| french | bonjour ! comment ça va ? | Bonjour ! Comment ça va ? |
| chinese | 你好 ，世界 。 | 你好，世界。 |
| turkish | istanbul güzel . iyi günler ! | İstanbul güzel. İyi günler! |
| english | where is the report | Where is the report |
| english | iPhone works. eBay too. | iPhone works. eBay too. |

Пример `where is the report` показывает ограничение: появилась заглавная буква, но вопросительный знак не добавился. `iPhone` и `eBay` сохраняют исходный регистр. Французские пробелы перед `!` и `?` сохраняются.

Источник: [проверяемые примеры](../tests/fixtures/text-processing/punctuation.json). Проверка: `text::punctuation::tests::published_examples_match_the_processor_and_are_idempotent`.

## 3. Только перевод

Ниже фактический результат **Hy-MT2-1.8B Q4_K_M** на CPU и CUDA. В этой серии ответы совпали; времена округлены до миллисекунд. Модель уже была загружена. Это демонстрационная выборка, а не доказательство качества на всём языке.

| Перевести на | До | После | CPU, мс | CUDA, мс |
|---|---|---|---|---|
| english | Отправь, пожалуйста, отчёт до пятницы. Спасибо! | Please send the report by Friday. Thank you! | 458 | 87 |
| russian | Please send the report by Friday. Thank you! | Пожалуйста, отправьте отчет до пятницы. Спасибо! | 792 | 147 |
| french | Please send the report by Friday. Thank you! | Veuillez envoyer le rapport avant vendredi. Merci ! | 517 | 91 |
| italian | Please send the report by Friday. Thank you! | Per favore invia il rapporto entro venerdì. Grazie! | 562 | 102 |
| spanish | Please send the report by Friday. Thank you! | Por favor, envíe el informe antes del viernes. Gracias. | 538 | 97 |
| chinese | Please send the report by Friday. Thank you! | 请在周五前提交报告。谢谢！ | 348 | 54 |
| english | Установи debug_mode=true в settings.toml и открой /api/v2/users. | Set debug_mode=true in settings.toml and open /api/v2/users. | 591 | 96 |
| english | Скажи «привет» и вернись завтра. | Say “hello” and come back tomorrow. | 385 | 59 |

В техническом примере сохранились `debug_mode=true`, `settings.toml` и `/api/v2/users`. Цитируемая проза переводится: «привет» становится “hello”. Формулировки, вежливость и финальная пунктуация могут изменяться: например, китайский ответ использует «提交» (представить/подать) для английского `send`. Когда точная формулировка важна, проверьте перевод и воспользуйтесь **Copy original transcript**.

Источники: [CPU](measurements/text-processing-2026-11-15/implementation-cpu.json), [CUDA](measurements/text-processing-2026-11-15/implementation-cuda.json), [нативный эксперимент](../crates/lw-app/examples/text_processing_smoke.rs).

## 4. Две очистки вместе

| До | После |
|---|---|
| Um, please send the report uh, by Friday . thank you ! | Please send the report by Friday. Thank you! |

Этот результат также проверяется `assert_eq!` в нативном эксперименте. Перевод в этом примере не включён. Если включить все три режима, сначала выполняются очистка и пользовательский словарь, затем перевод.

## Как повторить

```powershell
cargo test --locked -p lw-core text::
cargo run --locked -p lw-app --example text_processing_smoke -- .owlwhisp-test/text-feature-preview/settings.json cpu
# Вместо cpu можно указать cuda. При отсутствии файлов пример скачает переводчик.
python scripts/benchmarks/render_text_processing_examples.py
```

Все три опции по умолчанию выключены. Для перевода нужен отдельный скачиваемый локальный пакет. При ошибке или таймауте вставляется очищенный исходный текст; уведомление в Dictate объясняет причину. [Полный исследовательский отчёт](text-processing-research-2026-11-15.md).
