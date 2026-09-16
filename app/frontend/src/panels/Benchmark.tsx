import { useEffect, useRef, useState } from "react";
import {
  listBackends,
  listModels,
  onBenchmarkProgress,
  runBenchmark,
  runBenchmarkAll,
  type AcceleratorReport,
  type BackendOption,
  type BackendPreference,
  type BenchmarkProgress,
  type BenchReport,
  type BenchSuite,
  type ModelEntry,
} from "../ipc";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { formatMs, formatRtf, formatSeconds, formatWer } from "../format";
import { isCoarse, probeAccelerators, recordBenchRun, resolveOption } from "../backend";

/** "" means "leave it to Settings" for both selectors. */
type ModelChoice = string;
type BackendChoice = "" | BackendPreference;

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
                <th>cold RTF</th>
                <th>warm RTF</th>
                <th>WER</th>
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
  const [modelChoice, setModelChoice] = useState<ModelChoice>("");
  const [backendChoice, setBackendChoice] = useState<BackendChoice>("");
  const [report, setReport] = useState<BenchReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  // The sweep over every accelerator, kept in its own state so neither result can be mistaken for
  // the other.
  const [suite, setSuite] = useState<BenchSuite | null>(null);
  const [suiteError, setSuiteError] = useState<string | null>(null);
  const [runningAll, setRunningAll] = useState(false);
  const [progress, setProgress] = useState<BenchmarkProgress[]>([]);
  const [progressError, setProgressError] = useState<string | null>(null);

  const busy = running || runningAll;

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // The sweep's progress subscription. Registered *before* the command is invoked, so the first
  // backend's "running" event cannot be missed, and dropped the moment the sweep ends — or the
  // panel unmounts mid-run, which is what this ref is for.
  const unlisten = useRef<UnlistenFn | null>(null);
  const stopListening = () => {
    const fn = unlisten.current;
    unlisten.current = null;
    if (fn !== null) void fn();
  };
  useEffect(() => stopListening, []);

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

  // A visible clock, because a first NPU run can take minutes and silence looks like a hang.
  useEffect(() => {
    if (!busy) return;
    setElapsed(0);
    const started = Date.now();
    const timer = window.setInterval(() => {
      setElapsed(Math.round((Date.now() - started) / 1000));
    }, 1000);
    return () => {
      window.clearInterval(timer);
    };
  }, [busy]);

  const run = async () => {
    setRunning(true);
    setError(null);
    // One current result on screen: a sweep from five minutes ago sitting under a fresh single run
    // would read as part of it.
    setSuite(null);
    setSuiteError(null);
    setProgress([]);
    try {
      const result = await runBenchmark(
        modelChoice === "" ? undefined : modelChoice,
        backendChoice === "" ? undefined : backendChoice,
      );
      // File it for the Dictate panel: this run is the only proof of what actually executed.
      recordBenchRun(result, backendChoice === "" ? null : backendChoice);
      if (alive.current) setReport(result);
    } catch (e: unknown) {
      if (alive.current) setError(String(e));
    } finally {
      if (alive.current) setRunning(false);
    }
  };

  const runAll = async () => {
    setRunningAll(true);
    setSuiteError(null);
    setSuite(null);
    setProgress([]);
    setProgressError(null);
    setReport(null);
    setError(null);
    try {
      try {
        const fn = await onBenchmarkProgress((entry) => {
          if (!alive.current) return;
          setProgress((prev) => {
            const next = prev.filter((p) => p.accelerator !== entry.accelerator);
            next.push(entry);
            next.sort((a, b) => a.index - b.index);
            return next;
          });
        });
        if (alive.current) {
          unlisten.current = fn;
        } else {
          void fn();
        }
      } catch (e: unknown) {
        // The sweep still runs; only the running commentary is missing. Saying so beats a blank
        // screen for several minutes.
        if (alive.current) setProgressError(String(e));
      }
      const result = await runBenchmarkAll(modelChoice === "" ? undefined : modelChoice);
      if (alive.current) setSuite(result);
      // The sweep's last run is the last engine the worker held, and Dictate and Settings ask what
      // actually ran. Only filed when this build can name the preference that pinned it, so
      // nothing is attributed to a choice the user never made.
      const last = result.runs.length === 0 ? undefined : result.runs[result.runs.length - 1];
      if (last !== undefined) {
        const pinned =
          (backends ?? []).find(
            (o) => o.accelerator !== null && o.accelerator === last.accelerator,
          ) ?? null;
        if (pinned !== null) recordBenchRun(last, pinned.value);
      }
    } catch (e: unknown) {
      if (alive.current) setSuiteError(String(e));
    } finally {
      stopListening();
      if (alive.current) setRunningAll(false);
    }
  };

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
        <button className="btn" onClick={() => void run()} disabled={busy}>
          {running ? `Running… ${elapsed}s` : "Run benchmark"}
        </button>
        <button className="btn secondary" onClick={() => void runAll()} disabled={busy}>
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
          {report.clips.length === 0 ? (
            <p className="hint">The run produced no clips.</p>
          ) : (
            <table className="diag-table bench-table">
              <thead>
                <tr>
                  <th>clip</th>
                  <th>duration</th>
                  <th>time</th>
                  <th>RTF</th>
                  <th>WER</th>
                </tr>
              </thead>
              <tbody>
                {report.clips.map((clip, i) => (
                  <tr key={`${clip.name}-${i}`}>
                    <td>{clip.name}</td>
                    <td>{formatSeconds(clip.duration_s)}</td>
                    <td>{formatMs(clip.ms)}</td>
                    <td>{formatRtf(clip.rtf)}</td>
                    <td>{clip.wer === null ? "no reference" : formatWer(clip.wer)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          <h3 className="bench-heading">Measured on this machine</h3>
          <div className="bench-summary">
            <div className="stat">
              <span className="perf-label">Cold RTF</span>
              <span className="mono stat-value">{formatRtf(report.cold_rtf)}</span>
              <span className="sub">First run, including one-time warm-up.</span>
            </div>
            <div className="stat">
              <span className="perf-label">Warm RTF</span>
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
              <span className="perf-label">WER (word-weighted)</span>
              <span className="mono stat-value">
                {report.wer === null ? "—" : formatWer(report.wer)}
              </span>
              <span className="sub">
                {report.wer === null
                  ? "The clips had no reference transcripts, so accuracy could not be computed."
                  : "Word-weighted across all clips."}
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
