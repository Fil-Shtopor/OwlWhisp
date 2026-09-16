import { useEffect, useRef, useState } from "react";
import {
  listBackends,
  listModels,
  runBenchmark,
  type AcceleratorReport,
  type BackendOption,
  type BackendPreference,
  type BenchReport,
  type ModelEntry,
} from "../ipc";
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

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void listModels()
      .then((catalog) => {
        if (!disposed) {
          setModels(
            catalog.entries.filter((e) => e.runnable && e.install_state === "installed"),
          );
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
    if (!running) return;
    setElapsed(0);
    const started = Date.now();
    const timer = window.setInterval(() => {
      setElapsed(Math.round((Date.now() - started) / 1000));
    }, 1000);
    return () => {
      window.clearInterval(timer);
    };
  }, [running]);

  const run = async () => {
    setRunning(true);
    setError(null);
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
            disabled={running}
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
            disabled={running || backends === null}
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

      <div className="toolbar">
        <button className="btn" onClick={() => void run()} disabled={running}>
          {running ? `Running… ${elapsed}s` : "Run benchmark"}
        </button>
        <span className="sub hint">
          Takes tens of seconds. The first NPU run can take several minutes: the backend prepares and
          caches a context binary before it can time anything.
        </span>
      </div>

      {error !== null && <p className="status-err">Benchmark failed: {error}</p>}

      {report === null && !running && error === null && (
        <p className="hint">No benchmark has been run yet in this session.</p>
      )}

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
