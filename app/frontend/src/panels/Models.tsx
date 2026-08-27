import { useCallback, useEffect, useRef, useState } from "react";
import {
  cancelInstall,
  getCapabilities,
  getSettings,
  installModel,
  listModels,
  setSettings,
  type Capabilities,
  type InstallState,
  type MeasuredPoint,
  type ModelCatalog,
  type ModelEntry,
} from "../ipc";
import { formatBytes, formatEstimatedRtf, formatRtf, formatWer } from "../format";

/** Wording for each install state, plus the tone of its badge. */
const INSTALL_STATES: Record<InstallState, { label: string; hint: string; tone: string }> = {
  installed: {
    label: "Installed",
    hint: "The pinned file set is on disk and its hashes verified.",
    tone: "badge yes",
  },
  incomplete: {
    label: "Incomplete",
    hint: "Some pinned files are missing — downloading again resumes where it stopped.",
    tone: "badge warn",
  },
  missing: {
    label: "Not downloaded",
    hint: "Nothing on disk yet.",
    tone: "badge",
  },
  unpinned: {
    label: "Unpinned",
    hint: "No pinned manifest yet — cannot download from here.",
    tone: "badge no",
  },
};

/** Progress of the one install that can be in flight at a time. */
interface InstallProgress {
  id: string;
  file: string | null;
  received: number;
  total: number;
  verified: number;
  finishing: boolean;
}

function percent(p: InstallProgress): number {
  if (p.total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((p.received / p.total) * 100)));
}

function MeasuredLine({ point }: { point: MeasuredPoint }) {
  return (
    <div className="measured" title={`${point.machine} — ${point.source}`}>
      <span className="badge measured-badge">measured</span>{" "}
      <span className="mono">
        {point.hardware}: RTF {formatRtf(point.rtf)}
        {point.wer !== null && <> · WER {formatWer(point.wer)}</>}
      </span>
      <div className="sub">on {point.machine}</div>
    </div>
  );
}

export function ModelsPanel() {
  const [catalog, setCatalog] = useState<ModelCatalog | null>(null);
  const [caps, setCaps] = useState<Capabilities | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectBusy, setSelectBusy] = useState<string | null>(null);
  const [selectError, setSelectError] = useState<string | null>(null);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [installError, setInstallError] = useState<{ id: string; message: string } | null>(null);
  const [installNotice, setInstallNotice] = useState<{ id: string; message: string } | null>(null);
  const [cancelling, setCancelling] = useState(false);
  // Read inside the install callback/catch, where `cancelling` would be a stale closure value.
  const cancelRequested = useRef<string | null>(null);

  // Mirrors App.tsx's `disposed` flag, for callbacks that outlive a render.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await listModels();
      if (alive.current) {
        setCatalog(next);
        setLoadError(null);
      }
    } catch (e: unknown) {
      if (alive.current) setLoadError(String(e));
    } finally {
      if (alive.current) setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    void getCapabilities()
      .then((c) => {
        if (!disposed) setCaps(c);
      })
      .catch(() => {
        // Non-fatal: the catalog carries its own `machine` line.
      });
    void getSettings()
      .then((s) => {
        if (!disposed) setSelectedId(s.model_id);
      })
      .catch(() => {
        // Non-fatal: we simply cannot mark which entry is selected.
      });
    return () => {
      disposed = true;
    };
  }, []);

  const startInstall = async (id: string) => {
    setInstallError(null);
    setInstallNotice(null);
    setCancelling(false);
    cancelRequested.current = null;
    setProgress({ id, file: null, received: 0, total: 0, verified: 0, finishing: false });
    let refreshed = false;
    try {
      await installModel(id, (e) => {
        if (!alive.current) return;
        switch (e.event) {
          case "file_started":
            setProgress((p) =>
              p === null || p.id !== id ? p : { ...p, file: e.path, received: 0, total: e.total },
            );
            break;
          case "progress":
            setProgress((p) =>
              p === null || p.id !== id
                ? p
                : { ...p, file: e.path, received: e.received, total: e.total },
            );
            break;
          case "file_verified":
            setProgress((p) =>
              p === null || p.id !== id ? p : { ...p, verified: p.verified + 1 },
            );
            break;
          case "completed":
            setProgress((p) => (p === null || p.id !== id ? p : { ...p, finishing: true }));
            refreshed = true;
            void refresh();
            break;
          case "failed":
            setInstallError({ id, message: e.message });
            break;
        }
      });
      if (!refreshed) await refresh();
    } catch (e: unknown) {
      // A `failed` event carries the better message; only fall back to the rejection.
      if (alive.current) {
        setInstallError((prev) => (prev !== null && prev.id === id ? prev : { id, message: String(e) }));
      }
    } finally {
      if (alive.current) {
        setProgress(null);
        setCancelling(false);
      }
    }
  };

  const stopInstall = async (id: string) => {
    setCancelling(true);
    try {
      await cancelInstall(id);
    } catch (e: unknown) {
      if (alive.current) setInstallError({ id, message: String(e) });
    }
  };

  const select = async (id: string) => {
    setSelectBusy(id);
    setSelectError(null);
    try {
      const current = await getSettings();
      const saved = await setSettings({ ...current, model_id: id });
      if (alive.current) setSelectedId(saved.model_id);
    } catch (e: unknown) {
      if (alive.current) setSelectError(String(e));
    } finally {
      if (alive.current) setSelectBusy(null);
    }
  };

  if (loadError !== null && catalog === null) {
    return (
      <div>
        <p className="status-err">Failed to load the model catalog: {loadError}</p>
        <button className="btn secondary" onClick={() => void refresh()} disabled={refreshing}>
          {refreshing ? "Loading…" : "Try again"}
        </button>
      </div>
    );
  }
  if (catalog === null) {
    return <p className="hint">Loading the model catalog…</p>;
  }

  const renderEntry = (entry: ModelEntry) => {
    const isRecommended = catalog.recommended === entry.id;
    const isSelected = selectedId === entry.id;
    const state = INSTALL_STATES[entry.install_state];
    const installing = progress !== null && progress.id === entry.id;
    const otherInstalling = progress !== null && progress.id !== entry.id;
    const canDownload =
      entry.runnable && entry.install_state !== "unpinned" && entry.install_state !== "installed";
    const canSelect = entry.runnable && entry.install_state === "installed" && !isSelected;
    const error = installError !== null && installError.id === entry.id ? installError.message : null;

    return (
      <article
        key={entry.id}
        className={isRecommended ? "model-card recommended" : "model-card"}
      >
        <header className="model-head">
          <div>
            <h3 className="model-name">
              {entry.name}
              {isRecommended && (
                <span className="badge rec" title="Best fit for the machine described above">
                  Recommended
                </span>
              )}
              {isSelected && <span className="badge sel">Selected</span>}
            </h3>
            <p className="sub model-desc">{entry.description}</p>
          </div>
          <span className={state.tone} title={state.hint}>
            {state.label}
          </span>
        </header>

        <dl className="model-facts">
          <div>
            <dt>Engine</dt>
            <dd className="mono">{entry.engine}</dd>
          </div>
          <div>
            <dt>Download</dt>
            <dd>{formatBytes(entry.download_bytes)}</dd>
          </div>
          <div>
            <dt>On disk</dt>
            <dd>{formatBytes(entry.disk_bytes)}</dd>
          </div>
          <div>
            <dt>Languages</dt>
            <dd>{entry.language_summary}</dd>
          </div>
          <div>
            <dt>Quality</dt>
            <dd title="Editorial ranking, not a measurement">{entry.quality_label}</dd>
          </div>
          <div>
            <dt>Speed</dt>
            <dd title="The tier that feeds the RTF estimate">{entry.speed_label}</dd>
          </div>
          <div>
            <dt>Licence</dt>
            <dd>{entry.licence.length > 0 ? entry.licence : "—"}</dd>
          </div>
          <div>
            <dt>Hardware</dt>
            <dd>{entry.hardware.length > 0 ? entry.hardware.join(", ") : "—"}</dd>
          </div>
        </dl>

        <div className="model-perf">
          <div className="perf-block">
            <span className="perf-label">Estimated speed</span>
            <span className="mono estimate">{formatEstimatedRtf(entry.estimated_rtf)}</span>
            {entry.best_hardware !== null && (
              <span className="sub"> for {entry.best_hardware}</span>
            )}
          </div>
          <div className="perf-block">
            <span className="perf-label">Measured elsewhere</span>
            {entry.measured_reference === null ? (
              <span className="sub">No measurement in the catalog for this model.</span>
            ) : (
              <MeasuredLine point={entry.measured_reference} />
            )}
          </div>
        </div>

        {entry.measurements.length > 1 && (
          <details className="model-more">
            <summary>All catalog measurements ({entry.measurements.length})</summary>
            <div className="measured-list">
              {entry.measurements.map((m, i) => (
                <MeasuredLine key={`${m.hardware}-${i}`} point={m} />
              ))}
              {Object.keys(entry.wer_estimates).length > 0 && (
                <p className="sub">
                  WER estimates by language (published or small-sample, not a promise):{" "}
                  {Object.entries(entry.wer_estimates)
                    .map(([lang, wer]) => `${lang} ${formatWer(wer)}`)
                    .join(", ")}
                </p>
              )}
            </div>
          </details>
        )}

        <p className="sub model-reason">{entry.reason}</p>

        {!entry.runnable && entry.blockers.length > 0 && (
          <div className="blockers">
            <span className="perf-label">Cannot run here</span>
            <ul>
              {entry.blockers.map((b, i) => (
                <li key={i}>{b}</li>
              ))}
            </ul>
          </div>
        )}

        {entry.manifest_error !== null && (
          <p className="sub manifest-note">Manifest note: {entry.manifest_error}</p>
        )}
        {entry.notes !== null && entry.notes.length > 0 && (
          <details className="model-more">
            <summary>Notes</summary>
            <p className="sub notes-body">{entry.notes}</p>
          </details>
        )}
        {entry.install_dir !== null && (
          <p className="sub mono path">{entry.install_dir}</p>
        )}
        {entry.upstream_url !== null && (
          <p className="sub mono path">Upstream: {entry.upstream_url}</p>
        )}

        {installing && progress !== null && (
          <div className="install-progress">
            <div className="progress-bar">
              <div className="progress-fill" style={{ width: `${percent(progress)}%` }} />
            </div>
            <div className="sub progress-line">
              {progress.finishing
                ? "Finishing up…"
                : progress.total > 0
                  ? `${percent(progress)}% — ${formatBytes(progress.received)} of ${formatBytes(progress.total)}`
                  : "Starting…"}
              {progress.verified > 0 && ` · ${progress.verified} file(s) verified`}
            </div>
            {progress.file !== null && <div className="sub mono path">{progress.file}</div>}
          </div>
        )}

        <div className="model-actions">
          {installing ? (
            <button
              className="btn secondary"
              onClick={() => void stopInstall(entry.id)}
              disabled={cancelling || progress?.finishing === true}
            >
              {cancelling ? "Cancelling…" : "Cancel download"}
            </button>
          ) : (
            <button
              className="btn secondary"
              onClick={() => void startInstall(entry.id)}
              disabled={!canDownload || otherInstalling}
              title={
                entry.install_state === "unpinned"
                  ? "No pinned manifest yet — cannot download from here"
                  : entry.install_state === "installed"
                    ? "Already installed"
                    : !entry.runnable
                      ? "This build cannot run the model on this machine"
                      : undefined
              }
            >
              {entry.install_state === "incomplete" ? "Resume download" : "Download"}
            </button>
          )}
          <button
            className="btn"
            onClick={() => void select(entry.id)}
            disabled={!canSelect || selectBusy !== null}
            title={
              isSelected
                ? "Already the model in Settings"
                : entry.install_state !== "installed"
                  ? "Install it first"
                  : !entry.runnable
                    ? "This build cannot run the model on this machine"
                    : undefined
            }
          >
            {isSelected ? "In use" : selectBusy === entry.id ? "Saving…" : "Use this model"}
          </button>
        </div>

        {error !== null && <p className="status-err">Download failed: {error}</p>}
      </article>
    );
  };

  return (
    <div className="models">
      <section className="machine-box">
        <div className="perf-label">This machine</div>
        <div className="mono machine-line">{catalog.machine}</div>
        {caps !== null && (
          <div className="sub">
            {caps.cores} cores · {caps.os} {caps.arch} ·{" "}
            {caps.npu_usable
              ? `NPU usable${caps.npu_label !== null ? ` (${caps.npu_label})` : ""}`
              : caps.npu_present
                ? "NPU present but not usable by this build"
                : "no NPU detected"}
          </div>
        )}
        <div className="sub">
          Recommendations and estimates below are computed for this machine.
        </div>
      </section>

      <div className="toolbar">
        <button className="btn secondary" onClick={() => void refresh()} disabled={refreshing}>
          {refreshing ? "Refreshing…" : "Refresh"}
        </button>
        <span className="sub hint mono">{catalog.models_root}</span>
      </div>

      {loadError !== null && <p className="status-err">Refresh failed: {loadError}</p>}
      {selectError !== null && <p className="status-err">Could not save the model choice: {selectError}</p>}

      <p className="sub disclaimer">{catalog.estimate_disclaimer}</p>

      {catalog.entries.length === 0 ? (
        <p className="hint">The catalog is empty — no models to show.</p>
      ) : (
        <div className="model-list">{catalog.entries.map(renderEntry)}</div>
      )}
    </div>
  );
}
