import { useEffect, useState } from "react";
import {
  listBackends,
  listModels,
  type AcceleratorReport,
  type BackendOption,
  type BenchClip,
  type BenchmarkProgress,
  type BenchSuite,
  type ModelEntry,
} from "../ipc";
import { formatMs, formatRtf, formatSeconds, formatWer } from "../format";
import { isCoarse, probeAccelerators, resolveOption } from "../backend";
import {
  getBenchRun,
  setBackendChoice,
  setModelChoice,
  startBenchmark,
  startBenchmarkAll,
  subscribeBenchRun,
  type BackendChoice,
} from "../benchRun";

/** "" means "leave it to Settings" for both selectors; `BackendChoice` lives with the store. */
type ModelChoice = string;

/**
 * The option label as offered in the dropdown, with the reason it cannot be measured appended.
 *
 * Both halves come from the backend: the name from `BackendOption.label`, the reason from the
 * accelerator's own `detail`. Nothing here is spelled in the frontend, so a backend this build
 * gains can never show up mislabelled.
 */
function optionText(option: BackendOption, report: AcceleratorReport | null): string {
  const resolution = resolveOption(option, report);
  if (isCoarse(option) || resolution.availability === "usable") return option.label;
  if (resolution.availability === "unknown") return `${option.label} — availability unknown`;
  return `${option.label} — unavailable: ${resolution.reason}`;
}

/**
 * A note reports a fallback when the run did not get the backend it asked for — the one thing on
 * this panel the user must not miss. Deliberately a loose substring test: a note this fails to
 * classify is still rendered, just without the warning weight, so nothing is ever hidden.
 */
function isFallbackNote(note: string): boolean {
  const text = note.toLowerCase();
  return text.includes("unavailable") || text.includes("using cpu");
}

/**
 * What a run actually does, collapsed by default.
 *
 * Reference material rather than something to read before every run — but without it, "RTF 0.04,
 * WER 7.9%" is a pair of numbers with no method behind them, and the reader has no way to tell
 * that the cold figure is deliberately the worst one or that four of the twelve clips were left
 * out of the accuracy total on purpose.
 */
function Methodology() {
  return (
    <details className="bench-method">
      <summary>How this is measured</summary>
      <div className="bench-method-body">
        <p>
          A run transcribes the fixture clips committed with this repository — <strong>12 clips</strong>,
          three each in English, Russian, Spanish and Ukrainian — and times every one. They are real
          speech with reference transcripts, not a synthesised tone.
        </p>
        <dl className="bench-method-list">
          <div className="bench-method-item">
            <dt>
              RTF <span className="sub">real-time factor</span>
            </dt>
            <dd>
              Wall-clock seconds per second of audio; <strong>lower is faster</strong>, and 1.0
              means transcribing takes as long as the recording did. The <strong>first</strong>{" "}
              clip is reported separately as <em>cold</em>, because it carries the one-time warm-up.{" "}
              <em>Warm</em> is the mean of the rest, and warm is what steady-state dictation feels
              like.
            </dd>
          </div>
          <div className="bench-method-item">
            <dt>
              WER <span className="sub">word error rate</span>
            </dt>
            <dd>
              The share of words that came out wrong — substituted, dropped or invented — against
              the reference. <strong>Lower is better</strong>: 5% is about one word in twenty.
              Levenshtein distance over words, lowercased and
              with punctuation stripped, so casing and commas never count as errors. It is
              word-weighted across the clips — long clips carry more of the total — not a mean of
              per-clip rates.
            </dd>
          </div>
          <div className="bench-method-item">
            <dt>Which clips count</dt>
            <dd>
              WER is scored only on the languages the model claims. An English-only model is judged
              on the English clips; the rest are still transcribed and timed, but marked{" "}
              <span className="badge">not scored</span>, because a WER against a language a model
              never advertised measures the question rather than the model. Moonshine tiny en scores
              0.092 on English and 0.850 if you score it on all four.
            </dd>
          </div>
          <div className="bench-method-item">
            <dt>Comparing backends</dt>
            <dd>
              <strong>Compare all backends</strong> runs every usable accelerator over one clip set,
              loaded once before the sweep starts. That shared set is what makes the rows
              comparable.
            </dd>
          </div>
        </dl>
      </div>
    </details>
  );
}

/**
 * Every clip the model actually ran, timed.
 *
 * Clips in languages it does not claim are not here, because they were never transcribed — see the
 * line under the table, which says how many and in what. Running them would put the model's speed
 * at failing an unclaimed language into the same average as its speed at the job it is for.
 */
function ClipTable({
  clips,
  skipped,
  skippedLanguages,
}: {
  clips: readonly BenchClip[];
  skipped: number;
  skippedLanguages: readonly string[];
}) {
  if (clips.length === 0) {
    return <p className="hint">The run produced no clips.</p>;
  }
  const unscored = clips.filter((c) => !c.scored).length;
  return (
    <>
      <table className="diag-table bench-table clip-table">
        <thead>
          <tr>
            <th>clip</th>
            <th>language</th>
            <th>duration</th>
            <th>time</th>
            <th title="Real-time factor: seconds of computing per second of audio. Lower is faster.">RTF</th>
            <th title="Word error rate: the share of words that came out wrong. Lower is better.">WER</th>
          </tr>
        </thead>
        <tbody>
          {clips.map((clip, i) => (
            <tr key={`${clip.name}-${i}`} className={clip.scored ? "" : "clip-unscored"}>
              <td>{clip.name}</td>
              <td>{clip.language === null ? <span className="sub">unknown</span> : clip.language}</td>
              <td>{formatSeconds(clip.duration_s)}</td>
              <td>{formatMs(clip.ms)}</td>
              <td>{formatRtf(clip.rtf)}</td>
              <td className="clip-wer">
                {clip.wer === null ? "no reference" : formatWer(clip.wer)}
                {!clip.scored && (
                  <span
                    className="badge"
                    title="The model does not claim this clip's language, so its WER was kept out of the total."
                  >
                    not scored
                  </span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {skipped > 0 && (
        <p className="sub">
          <strong>
            {skipped} further clip{skipped === 1 ? "" : "s"}
          </strong>{" "}
          {skippedLanguages.length > 0 && <>in {skippedLanguages.join(", ")} </>}
          {skipped === 1 ? "was" : "were"} <strong>not run</strong>: this model does not claim{" "}
          {skipped === 1 ? "that language" : "those languages"}. Nothing above includes them —
          neither the accuracy nor the timings — which is the point. A model asked to transcribe a
          language it was never built for still takes time to produce something wrong, and averaging
          that in would report a speed nobody will ever see on the audio the model is for.
        </p>
      )}
      {unscored > 0 && (
        <p className="sub">
          {unscored} of {clips.length} clips are marked <strong>not scored</strong>: this model does
          not claim their language, so they were transcribed and timed — they count towards RTF —
          but kept out of the WER total. Their own WER is still shown above.
        </p>
      )}
    </>
  );
}

/** One line of running commentary while a sweep is in flight. */
function ProgressRow({ entry }: { entry: BenchmarkProgress }) {
  const warm = entry.warm_rtf;
  const wer = entry.wer;
  return (
    <li className={`bench-progress-row ${entry.stage}`}>
      <span className="mono bench-progress-index">
        {entry.index + 1}/{entry.total}
      </span>
      <span className="bench-progress-label">{entry.label}</span>
      {entry.stage === "running" && <span className="badge warn">measuring…</span>}
      {entry.stage === "done" && <span className="badge yes">done</span>}
      {entry.stage === "failed" && <span className="badge no">failed</span>}
      {entry.stage === "done" && (
        <span className="sub mono">
          warm {warm === null || warm === undefined ? "—" : formatRtf(warm)} · WER{" "}
          {wer === null || wer === undefined ? "—" : formatWer(wer)}
        </span>
      )}
      {entry.stage === "failed" && <span className="sub">{entry.reason ?? "no reason given"}</span>}
    </li>
  );
}

/**
 * The comparison: one row per accelerator that ran, all on one clip set.
 *
 * `fastest` and `most_accurate` measure two different things and are routinely two different
 * rows, so they are marked distinctly and never collapsed into a single "winner".
 */
function SuiteReport({ suite }: { suite: BenchSuite }) {
  const fastestRan = suite.runs.some(
    (r) => r.accelerator !== null && r.accelerator === suite.fastest,
  );
  const accurateRan = suite.runs.some(
    (r) => r.accelerator !== null && r.accelerator === suite.most_accurate,
  );

  return (
    <div className="bench-report">
      <h3 className="bench-heading">Every backend, side by side</h3>
      <table className="diag-table">
        <tbody>
          <tr>
            <th>machine</th>
            <td>{suite.machine}</td>
          </tr>
          <tr>
            <th>model</th>
            <td>{suite.model_id}</td>
          </tr>
          <tr>
            <th>clip source</th>
            <td>{suite.clip_source}</td>
          </tr>
        </tbody>
      </table>
      <p className="sub">
        Every row below ran <strong>those same clips</strong>, loaded once before the sweep started.
        That is what makes these numbers a comparison rather than three unrelated benchmarks.
      </p>

      {suite.runs.length === 0 ? (
        <p className="status-err">
          No accelerator could be measured. Anything that was offered is listed below with the
          reason it did not run.
        </p>
      ) : (
        <>
          <table className="diag-table bench-table compare-table">
            <thead>
              <tr>
                <th>backend</th>
                <th title="Real-time factor on the first clip, which carries the one-time warm-up. Lower is faster.">cold RTF</th>
                <th title="Mean real-time factor over the clips after the first. Lower is faster.">warm RTF</th>
                <th title="Word error rate: the share of words that came out wrong. Lower is better.">WER</th>
                <th>marks</th>
              </tr>
            </thead>
            <tbody>
              {suite.runs.map((run, i) => {
                const id = run.accelerator;
                const fastest = id !== null && id === suite.fastest;
                const accurate = id !== null && id === suite.most_accurate;
                const classes = ["compare-row"];
                if (fastest) classes.push("fastest");
                if (accurate) classes.push("accurate");
                return (
                  <tr key={`${id ?? run.backend}-${i}`} className={classes.join(" ")}>
                    <td>
                      <span className="accel-label">{run.backend}</span>
                      {id !== null && <span className="sub mono"> {id}</span>}
                    </td>
                    <td>{formatRtf(run.cold_rtf)}</td>
                    <td>{run.warm_rtf === null ? "no warm run" : formatRtf(run.warm_rtf)}</td>
                    <td>{run.wer === null ? "no reference" : formatWer(run.wer)}</td>
                    <td className="compare-marks">
                      {fastest && <span className="badge fastest">fastest</span>}
                      {accurate && <span className="badge accurate">most accurate</span>}
                      {!fastest && !accurate && <span className="sub">—</span>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          <p className="sub">
            <strong>fastest</strong> is the lowest <strong>warm</strong> RTF — the steady-state
            figure, after the one-time warm-up that inflates the cold column.{" "}
            <strong>most accurate</strong> is the lowest WER. Two different measurements, so they
            are often two different rows.
          </p>
          {suite.fastest !== null && !fastestRan && (
            <p className="sub">
              The fastest accelerator is reported as <span className="mono">{suite.fastest}</span>,
              which is not one of the rows above.
            </p>
          )}
          {suite.most_accurate !== null && !accurateRan && (
            <p className="sub">
              The most accurate accelerator is reported as{" "}
              <span className="mono">{suite.most_accurate}</span>, which is not one of the rows
              above.
            </p>
          )}
          {suite.fastest === null && (
            <p className="sub">
              No run is marked fastest — the backend named none, so no warm figure could be ranked.
            </p>
          )}
          {suite.most_accurate === null && (
            <p className="sub">
              No run is marked most accurate — with no reference transcripts on the clips, WER
              cannot be computed and accuracy cannot be ranked.
            </p>
          )}
        </>
      )}

      {suite.skipped.length > 0 && (
        <>
          <h3 className="bench-heading">Not measured</h3>
          <p className="sub">
            These accelerators were offered but produced no number. An accelerator that could not
            run is information, so it is shown rather than dropped.
          </p>
          <table className="diag-table">
            <tbody>
              {suite.skipped.map((skip, i) => (
                <tr key={`${skip.accelerator}-${i}`}>
                  <th>{skip.label}</th>
                  <td>{skip.reason}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}

      {suite.runs.some((run) => run.notes.length > 0) && (
        <div className="bench-notes">
          <h3 className="bench-heading">Backend selection</h3>
          <p className="sub">
            What each engine decided while selecting its backend, in the order it decided it.
          </p>
          {suite.runs
            .filter((run) => run.notes.length > 0)
            .map((run, i) => (
              <details key={`${run.accelerator ?? run.backend}-${i}`} className="bench-run-notes">
                <summary>{run.backend}</summary>
                <ul className="bench-note-list">
                  {run.notes.map((note, j) => {
                    const fallback = isFallbackNote(note);
                    return (
                      <li
                        key={`${j}-${note}`}
                        className={fallback ? "bench-note fallback" : "bench-note"}
                      >
                        <span className={fallback ? "badge warn" : "badge"}>
                          {fallback ? "fallback" : "note"}
                        </span>
                        <span className="bench-note-text">{note}</span>
                      </li>
                    );
                  })}
                </ul>
              </details>
            ))}
        </div>
      )}
    </div>
  );
}

export function BenchmarkPanel() {
  const [models, setModels] = useState<ModelEntry[] | null>(null);
  // Null until `list_backends` answers: the labels are the Rust side's to give.
  const [backends, setBackends] = useState<BackendOption[] | null>(null);
  const [backendsError, setBackendsError] = useState<string | null>(null);
  const [accel, setAccel] = useState<AcceleratorReport | null>(null);
  /**
   * The run itself lives in a module-level store, not here.
   *
   * A sweep takes minutes and App.tsx unmounts this panel on any tab change. Held locally, the
   * progress list, the clock and the result all died with the component while the backend carried
   * on — so a run in flight looked cancelled, and a result that landed while the user was
   * elsewhere was lost. This component only renders the store and asks it to start things.
   */
  const [run, setRun] = useState(getBenchRun);
  const [elapsed, setElapsed] = useState(0);

  const { report, suite, error, suiteError, progress, progressError } = run;
  const running = run.activity === "single";
  const runningAll = run.activity === "sweep";
  const busy = run.activity !== null;
  const modelChoice: ModelChoice = run.modelChoice;
  const backendChoice: BackendChoice = run.backendChoice;

  useEffect(() => subscribeBenchRun(setRun), []);

  useEffect(() => {
    let disposed = false;
    void listModels()
      .then((catalog) => {
        if (!disposed) {
          setModels(catalog.entries.filter((e) => e.runnable && e.install_state === "installed"));
        }
      })
      .catch(() => {
        if (!disposed) setModels([]);
      });
    void listBackends()
      .then((b) => {
        if (!disposed) setBackends(b);
      })
      .catch((e: unknown) => {
        if (!disposed) setBackendsError(String(e));
      });
    // Used only to mark the ones this machine cannot measure — a run forced onto an unusable
    // provider is a strict failure, not a slower number.
    void probeAccelerators()
      .then((report) => {
        if (!disposed) setAccel(report);
      })
      .catch(() => {
        // Without it every option stays selectable, which is the safe direction: an unprobed
        // accelerator is unknown, not unavailable.
      });
    return () => {
      disposed = true;
    };
  }, []);

  // A visible clock, because a first NPU run can take minutes and silence looks like a hang. It is
  // derived from the moment the run started rather than counted from zero, so coming back to this
  // tab after four minutes shows four minutes.
  useEffect(() => {
    const startedAt = run.startedAt;
    if (startedAt === null) return;
    const tick = () => setElapsed(Math.max(0, Math.round((Date.now() - startedAt) / 1000)));
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => {
      window.clearInterval(timer);
    };
  }, [run.startedAt]);

  const options = backends ?? [];
  const coarseOptions = options.filter(isCoarse);
  const exactOptions = options.filter((o) => !isCoarse(o));
  const renderOption = (option: BackendOption) => {
    const resolution = resolveOption(option, accel);
    // Coarse choices stay selectable: "any NPU" is a legitimate thing to measure the failure of.
    const blocked =
      !isCoarse(option) &&
      (resolution.availability === "unusable" || resolution.availability === "unsupported");
    return (
      <option key={option.value} value={option.value} disabled={blocked}>
        {optionText(option, accel)}
      </option>
    );
  };

  const nothingYet =
    report === null && suite === null && !busy && error === null && suiteError === null;

  return (
    <div className="bench">
      <p className="hint">
        Every number on this page is measured on this machine by the run you start here — unlike the
        catalog figures in Models, which are estimates.
      </p>

      <Methodology />

      <div className="bench-controls">
        <div className="field">
          <label htmlFor="bench-model">Model</label>
          <select
            id="bench-model"
            value={modelChoice}
            onChange={(e) => setModelChoice(e.currentTarget.value)}
            disabled={busy}
          >
            <option value="">From Settings</option>
            {(models ?? []).map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
          <span className="sub">
            {models === null
              ? "Loading installed models…"
              : models.length === 0
                ? "No installed, runnable model found — the run falls back to the one in Settings."
                : "Only installed models this build can run are listed."}
          </span>
          <span className="sub">Both actions below measure this model.</span>
        </div>

        <div className="field">
          <label htmlFor="bench-backend">Backend</label>
          <select
            id="bench-backend"
            value={backendChoice}
            onChange={(e) => {
              const chosen = options.find((o) => o.value === e.currentTarget.value) ?? null;
              setBackendChoice(chosen === null ? "" : chosen.value);
            }}
            disabled={busy || backends === null}
          >
            <option value="">From Settings</option>
            {coarseOptions.length > 0 && (
              <optgroup label="By kind of hardware">{coarseOptions.map(renderOption)}</optgroup>
            )}
            {exactOptions.length > 0 && (
              <optgroup label="One exact provider">{exactOptions.map(renderOption)}</optgroup>
            )}
          </select>
          <span className="sub">
            Forcing a backend is how you confirm (or refute) an estimate: run each one and compare
            the measured RTF. Providers this machine cannot use are listed with the reason and
            cannot be selected — a strict choice fails rather than producing a slower number.
          </span>
          <span className="sub">
            This applies to <strong>Run benchmark</strong> only.{" "}
            <strong>Compare all backends</strong> measures every usable accelerator, so it ignores
            this choice.
          </span>
          {backends === null && backendsError === null && (
            <span className="sub">Loading backends…</span>
          )}
          {backendsError !== null && (
            <span className="status-err">
              Could not read the list of backends: {backendsError}. Runs can still use the one in
              Settings.
            </span>
          )}
        </div>
      </div>

      <div className="toolbar bench-actions">
        <button className="btn" onClick={() => void startBenchmark()} disabled={busy}>
          {running ? `Running… ${elapsed}s` : "Run benchmark"}
        </button>
        <button className="btn secondary" onClick={() => void startBenchmarkAll(backends ?? [])} disabled={busy}>
          {runningAll ? `Comparing… ${elapsed}s` : "Compare all backends"}
        </button>
      </div>
      <p className="sub hint bench-timing">
        <strong>Run benchmark</strong> measures one backend and takes tens of seconds.{" "}
        <strong>Compare all backends</strong> measures CPU, GPU and NPU on one shared clip set in a
        single action and <strong>takes several minutes</strong>: each backend loads its own engine,
        and a first NPU run also prepares and caches a context binary before it can time anything.
        Both drop the dictation engine; the next dictation loads it again.
      </p>

      {error !== null && <p className="status-err">Benchmark failed: {error}</p>}
      {suiteError !== null && <p className="status-err">Comparison failed: {suiteError}</p>}

      {nothingYet && <p className="hint">No benchmark has been run yet in this session.</p>}

      {(runningAll || (suite === null && progress.length > 0)) && (
        <div className="bench-progress">
          <h3 className="bench-heading">
            {runningAll ? "Measuring every usable accelerator" : "How far the comparison got"}
          </h3>
          {progressError !== null && (
            <p className="sub">
              Progress updates are unavailable ({progressError}), so this sweep can only report when
              it finishes. It is still running.
            </p>
          )}
          {progress.length === 0 ? (
            <p className="hint">
              Preparing: loading the clip set every backend will share, and listing the accelerators
              to measure. Nothing is reported until the first one starts.
            </p>
          ) : (
            <ol className="bench-progress-list">
              {progress.map((entry) => (
                <ProgressRow key={entry.accelerator} entry={entry} />
              ))}
            </ol>
          )}
        </div>
      )}

      {suite !== null && <SuiteReport suite={suite} />}

      {report !== null && (
        <div className="bench-report">
          <table className="diag-table">
            <tbody>
              <tr>
                <th>machine</th>
                <td>{report.machine}</td>
              </tr>
              <tr>
                <th>backend that ran</th>
                <td>{report.backend}</td>
              </tr>
              <tr>
                <th>model</th>
                <td>{report.model_id}</td>
              </tr>
              <tr>
                <th>model dir</th>
                <td>{report.model_dir}</td>
              </tr>
              <tr>
                <th>clip source</th>
                <td>{report.clip_source}</td>
              </tr>
              <tr>
                <th>engine load</th>
                <td>{formatMs(report.engine_load_ms)}</td>
              </tr>
              <tr>
                <th>audio</th>
                <td>{formatSeconds(report.audio_secs)}</td>
              </tr>
            </tbody>
          </table>

          {report.notes.length > 0 && (
            <div className="bench-notes">
              <h3 className="bench-heading">Backend selection</h3>
              <p className="sub">
                “backend that ran” above is what actually executed; these are the engine’s notes on
                why, in the order it decided them.
              </p>
              <ul className="bench-note-list">
                {report.notes.map((note, i) => {
                  const fallback = isFallbackNote(note);
                  return (
                    <li
                      key={`${i}-${note}`}
                      className={fallback ? "bench-note fallback" : "bench-note"}
                    >
                      <span className={fallback ? "badge warn" : "badge"}>
                        {fallback ? "fallback" : "note"}
                      </span>
                      <span className="bench-note-text">{note}</span>
                    </li>
                  );
                })}
              </ul>
            </div>
          )}

          <h3 className="bench-heading">Per clip</h3>
          <ClipTable
            clips={report.clips}
            skipped={report.skipped_clips}
            skippedLanguages={report.skipped_languages}
          />

          <h3 className="bench-heading">Measured on this machine</h3>
          <div className="bench-summary">
            <div className="stat">
              <span className="perf-label" title="Real-time factor: seconds of computing per second of audio. Lower is faster.">
                Cold RTF <span className="sub">real-time factor, lower is faster</span>
              </span>
              <span className="mono stat-value">{formatRtf(report.cold_rtf)}</span>
              <span className="sub">First run, including one-time warm-up.</span>
            </div>
            <div className="stat">
              <span className="perf-label" title="Real-time factor: seconds of computing per second of audio. Lower is faster.">
                Warm RTF <span className="sub">real-time factor, lower is faster</span>
              </span>
              <span className="mono stat-value">
                {report.warm_rtf === null ? "no warm run" : formatRtf(report.warm_rtf)}
              </span>
              <span className="sub">
                {report.warm_rtf === null
                  ? "Only one clip, so nothing ran after the first — there is no warm figure to average."
                  : `Mean of ${report.warm_count} run${report.warm_count === 1 ? "" : "s"} after the first.`}
              </span>
            </div>
            <div className="stat">
              <span
                className="perf-label"
                title="The share of words — or, for languages written without spaces, characters — that came out wrong. Lower is better."
              >
                {report.unit ?? "Accuracy"}{" "}
                <span className="sub">
                  {report.unit === "CER"
                    ? "character error rate, lower is better"
                    : "word error rate, lower is better"}
                </span>
              </span>
              <span className="mono stat-value">
                {report.wer === null ? "—" : formatWer(report.wer)}
              </span>
              <span className="sub">
                {/*
                  Three different reasons for having no figure, and they are not the same thing.
                  Mixing units is the newest: a run covering Russian and Chinese has no single
                  number, because a word rate and a character rate cannot be averaged.
                */}
                {report.mixed_units
                  ? "No single figure: these clips span languages scored in different units — words for most, characters for the ones written without spaces — and the two cannot be averaged. The per-language breakdown above is the answer."
                  : report.wer === null
                    ? "No clip contributed a reference this model could be scored against, so accuracy could not be computed."
                    : report.clips.every((c) => c.scored)
                      ? "Token-weighted across all clips."
                      : `Token-weighted across the ${report.clips.filter((c) => c.scored).length} clips in languages this model claims.`}
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
