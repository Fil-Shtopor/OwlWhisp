# OwlWhisp 0.2.1 preview

Dictation can now remove explicit hesitation sounds, tidy punctuation and capitalization,
and translate into a selected language. All three settings are independent and off by default.
Open **Settings → Text processing**, expand **How these options work** for the limitations,
then click **Save** to apply your choices.

- **Remove filler sounds** uses conservative rules for 17 languages: English, Russian,
  Ukrainian, Spanish, Italian, French, German, Portuguese, Dutch, Polish, Czech, Turkish,
  Chinese, Japanese, Korean, Arabic and Hindi. Choose the cleanup language manually when
  automatic detection is uncertain. Quotes, code, links, paths and numeric forms are protected.
  Meaningful or ambiguous words such as `like`, `well`, `ну`, `типа`, `那个` and `嗯` remain.
- **Punctuation and capitalization** tidies spaces before existing punctuation and capitalizes
  sentence starts without another model. It does not infer missing commas or rewrite grammar.
  Turning it off preserves punctuation and case supplied by speech recognition or translation.
- **Translate dictation** uses optional local Hy-MT2-1.8B Q4_K_M with a downloadable,
  SHA-256-verified llama.cpp runtime. Select the target language, CPU or NVIDIA CUDA, and
  maximum extra wait. Translation stays on the device. Runtime downloads require internet;
  the separately selected speech recognition engine still determines where audio is processed.
- **Recover the source:** on timeout or translation failure, the cleaned source text is kept
  and Dictate shows the reason. **Copy original transcript** recovers the recognizer's text
  whenever processing changed it. Translation workers follow the background idle-unload setting.

Cleanup needs no extra model weights. The translation model is 1.13 GB, plus runtime and
installation archives. On the tested RTX 4080 Laptop, warm translation of eight short examples
took 0.05–0.15 seconds; CPU took 0.35–0.8 seconds. These are text-stage measurements, excluding
speech recognition; loading and longer dictation take additional time. The default extra-wait
budget is 5 seconds. CUDA requires a compatible NVIDIA driver; CPU remains available explicitly.

See the [before-and-after examples](https://github.com/Fil-Shtopor/OwlWhisp/blob/v0.2.1/docs/text-processing-examples-v0.2.1.md)
and [full experiment report](https://github.com/Fil-Shtopor/OwlWhisp/blob/v0.2.1/docs/text-processing-research-2026-11-15.md).

The release workflow verifies all six desktop platforms, the complete set of 14 release assets,
and public downloads against SHA-256 checksums. This remains an unsigned preview release.
macOS/Linux desktop integration and RTX Spark retain their existing preview limitations.
