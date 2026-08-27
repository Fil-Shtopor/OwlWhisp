import { useEffect, useRef, useState } from "react";
import {
  listBackends,
  listModels,
  runBenchmark,
  type BackendPreference,
  type BenchReport,
  type ModelEntry,
} from "../ipc";
import { formatMs, formatRtf, formatSeconds, formatWer } from "../format";

const BACKEND_LABELS: Record<BackendPreference, string> = {
  automatic: "Automatic",
  force_npu: "NPU",
  force_cpu: "CPU",
};

/** "" means "leave it to Settings" for both selectors. */
type ModelChoice = string;
type BackendChoice = "" | BackendPreference;

function isBackendPreference(value: string): value is BackendPreference {
  return value === "automatic" || value === "force_npu" || value === "force_cpu";
}

export function BenchmarkPanel() {
  const [models, setModels] = useState<ModelEntry[] | null>(null);
  const [backends, setBackends] = useState<BackendPreference[]>([
    "automatic",
    "force_npu",
    "force_cpu",
  ]);
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
        if (!disposed && b.length > 0) setBackends(b);
      })
      .catch(() => {
        // keep the static fallback list
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
      if (alive.current) setReport(result);
    } catch (e: unknown) {
      if (alive.current) setError(String(e));
    } finally {
      if (alive.current) setRunning(false);
    }
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
              const v = e.currentTarget.value;
              setBackendChoice(v === "" ? "" : isBackendPreference(v) ? v : "");
            }}
            disabled={running}
          >
            <option value="">From Settings</option>
            {backends.map((b) => (
              <option key={b} value={b}>
                {BACKEND_LABELS[b]}
              </option>
            ))}
          </select>
          <span className="sub">Forcing a backend is how you confirm (or refute) an estimate.</span>
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
              <span className="mono stat-value">{formatRtf(report.warm_rtf)}</span>
              <span className="sub">
                Mean of {report.warm_count} run{report.warm_count === 1 ? "" : "s"} after the first.
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
