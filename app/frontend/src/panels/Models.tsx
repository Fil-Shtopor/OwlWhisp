import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getCapabilities,
  getSettings,
  listModels,
  setSettings,
  type Capabilities,
  type InstallState,
  type MeasuredPoint,
  type ModelCatalog,
  type ModelEntry,
  type QualityTier,
} from "../ipc";
import {
  formatBytes,
  formatEstimatedRtf,
  formatEstimatedRtfCompact,
  formatRtf,
  formatWer,
} from "../format";
import {
  getInstall,
  installPercent,
  onInstallSettled,
  startInstall,
  stopInstall,
  subscribeInstall,
  type InstallProgress,
} from "../installRun";

/**
 * Wording for each install state, plus the tone of its badge.
 *
 * `short` is what fits the table column; `label` and `hint` are the full wording, which stays
 * reachable through the badge's `title` and the expanded detail.
 */
const INSTALL_STATES: Record<
  InstallState,
  { label: string; short: string; hint: string; tone: string }
> = {
  installed: {
    label: "Installed",
    short: "Installed",
    hint: "The pinned file set is on disk and its hashes verified.",
    tone: "badge yes",
  },
  incomplete: {
    label: "Incomplete",
    short: "Partial",
    hint: "Some pinned files are missing — downloading again resumes where it stopped.",
    tone: "badge warn",
  },
  missing: {
    label: "Not downloaded",
    short: "Missing",
    hint: "Nothing on disk yet.",
    tone: "badge",
  },
  unpinned: {
    label: "Unpinned",
    short: "Unpinned",
    hint: "No pinned manifest yet — cannot download from here.",
    tone: "badge no",
  },
};

/**
 * Display names for engine ids the catalog is known to use.
 *
 * `EngineKind` round-trips anything it does not recognise as a plain string, and the catalog is
 * being widened, so an unknown id must still render: `familyLabel` title-cases it rather than
 * dropping it. The raw id always stays in the cell's `title` and in the expanded facts.
 */
const ENGINE_FAMILY_LABELS: Readonly<Record<string, string | undefined>> = {
  parakeet_tdt: "Parakeet",
  nemo_transducer: "NeMo",
  nemo_ctc: "NeMo",
  whisper: "Whisper",
  moonshine: "Moonshine",
  sense_voice: "SenseVoice",
  paraformer: "Paraformer",
  zipformer: "Zipformer",
  telespeech: "TeleSpeech",
  fire_red_asr: "FireRedASR",
  dolphin: "Dolphin",
  canary: "Canary",
  wenet_ctc: "WeNet",
};

/**
 * A hardware label short enough to sit inside a column. Only the QNN spelling needs shortening;
 * anything else is passed through, and the full string is always in the `title`.
 */
function shortHardware(hardware: string): string {
  const lower = hardware.toLowerCase();
  return lower === "qnn_npu" || lower === "qnn-npu" ? "npu" : hardware;
}

function familyLabel(engine: string): string {
  const known = ENGINE_FAMILY_LABELS[engine];
  if (known !== undefined) return known;
  const words = engine.split("_").filter((w) => w.length > 0);
  if (words.length === 0) return "—";
  return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ");
}

/**
 * The language cell. A 25-language list cannot be a column, so the count stands in for it and the
 * catalog's own `language_summary` goes in the `title`; the full list is in the expansion.
 */
function compactLanguages(entry: ModelEntry): string {
  const langs = entry.languages;
  if (langs.length === 0) return "—";
  if (langs.length <= 2) return langs.join(", ");
  return `${langs.length} langs`;
}

/** A measured word-error rate the row can show, and whether it describes this machine's target. */
interface AccuracyEvidence {
  readonly point: MeasuredPoint;
  readonly wer: number;
  /** True when it was taken on the hardware this machine would actually use. */
  readonly onBestHardware: boolean;
}

/**
 * The best measured WER the catalog holds for an entry.
 *
 * `measured_reference` is only the measurement for `best_hardware`, so it is null for a model
 * measured on the CPU alone when this machine would use the NPU — while `measurements` still
 * holds a perfectly real number. Reading only the reference would make such a model look
 * *less* evidenced than one that happens to have been measured on the matching target, so fall
 * back to the rest of the measurements and let the caller say which hardware it came from.
 */
function measuredAccuracy(entry: ModelEntry): AccuracyEvidence | null {
  const ref = entry.measured_reference;
  if (ref !== null && ref.wer !== null) {
    return { point: ref, wer: ref.wer, onBestHardware: true };
  }
  let bestPoint: MeasuredPoint | null = null;
  let bestWer: number | null = null;
  for (const m of entry.measurements) {
    const wer = m.wer;
    if (wer === null) continue;
    if (bestWer === null || wer < bestWer) {
      bestPoint = m;
      bestWer = wer;
    }
  }
  if (bestPoint === null || bestWer === null) return null;
  return {
    point: bestPoint,
    wer: bestWer,
    // Hardware spellings differ between the two fields (`qnn_npu` vs `qnn-npu`), so the only
    // sound comparison is against the reference's own hardware string.
    onBestHardware: ref !== null && bestPoint.hardware === ref.hardware,
  };
}

const QUALITY_RANK: Record<QualityTier, number> = { basic: 0, good: 1, better: 2, best: 3 };
const STATE_RANK: Record<InstallState, number> = {
  installed: 0,
  incomplete: 1,
  missing: 2,
  unpinned: 3,
};

// ---------------------------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------------------------

type SortKey = "name" | "family" | "size" | "languages" | "speed" | "accuracy" | "state";
type SortDir = "asc" | "desc";

interface SortColumn {
  /** Spelled into the header button's tooltip. */
  readonly title: string;
  readonly defaultDir: SortDir;
  /** Rows with no value for this column sort last whichever way the arrow points. */
  readonly missing: (e: ModelEntry) => boolean;
  /** Ascending comparator over one published field. Deliberately never a composite score. */
  readonly compare: (a: ModelEntry, b: ModelEntry) => number;
}

const SORT_COLUMNS: Readonly<Record<SortKey, SortColumn>> = {
  name: {
    title: "name",
    defaultDir: "asc",
    missing: () => false,
    compare: (a, b) => a.name.localeCompare(b.name),
  },
  family: {
    title: "engine family",
    defaultDir: "asc",
    missing: () => false,
    compare: (a, b) => familyLabel(a.engine).localeCompare(familyLabel(b.engine)),
  },
  size: {
    title: "download size",
    defaultDir: "asc",
    missing: (e) => e.download_bytes === null,
    compare: (a, b) => (a.download_bytes ?? 0) - (b.download_bytes ?? 0),
  },
  languages: {
    title: "number of languages",
    defaultDir: "desc",
    missing: () => false,
    compare: (a, b) => a.languages.length - b.languages.length,
  },
  speed: {
    title: "estimated RTF",
    defaultDir: "asc",
    missing: (e) => e.estimated_rtf === null,
    compare: (a, b) => (a.estimated_rtf ?? 0) - (b.estimated_rtf ?? 0),
  },
  accuracy: {
    // The tier only. Measured WER exists for a minority of entries, so mixing it in would rank
    // models against each other on data most of them do not have.
    title: "quality tier",
    defaultDir: "desc",
    missing: () => false,
    compare: (a, b) => QUALITY_RANK[a.quality] - QUALITY_RANK[b.quality],
  },
  state: {
    title: "install state",
    defaultDir: "asc",
    missing: () => false,
    compare: (a, b) => STATE_RANK[a.install_state] - STATE_RANK[b.install_state],
  },
};

// ---------------------------------------------------------------------------------------------
// Grouping by maker
// ---------------------------------------------------------------------------------------------

/** Where an entry with no recorded maker goes. A residue, never a company. */
const OTHER_VENDOR = "Other";

interface VendorGroup {
  readonly vendor: string;
  readonly entries: readonly ModelEntry[];
}

/**
 * One continuous list, split by who made each model.
 *
 * Deliberately not a filter: every model stays on screen at once, and a heading plus a little
 * space is the whole of the grouping. Vendor tabs would hide most of the catalog behind a click,
 * which is the opposite of what a comparison table is for.
 *
 * Order: the recommended model's maker leads, so the recommendation stays near the top; then the
 * makers offering the most models, alphabetically within a tie; then `Other`. It depends only on
 * the catalog, so it does not shuffle when a model is installed or selected. Within a group the
 * catalog's own order is kept untouched.
 */
function groupByVendor(entries: readonly ModelEntry[], recommended: string | null): VendorGroup[] {
  const buckets = new Map<string, ModelEntry[]>();
  for (const entry of entries) {
    const name =
      entry.vendor === null || entry.vendor.trim() === "" ? OTHER_VENDOR : entry.vendor.trim();
    const bucket = buckets.get(name);
    if (bucket === undefined) buckets.set(name, [entry]);
    else bucket.push(entry);
  }

  const recommendedEntry =
    recommended === null ? undefined : entries.find((e) => e.id === recommended);
  const leadVendor =
    recommendedEntry === undefined || recommendedEntry.vendor === null
      ? null
      : recommendedEntry.vendor.trim();

  return [...buckets.entries()]
    .map(([vendor, list]) => ({ vendor, entries: list }))
    .sort((a, b) => {
      if (a.vendor === b.vendor) return 0;
      if (leadVendor !== null) {
        if (a.vendor === leadVendor) return -1;
        if (b.vendor === leadVendor) return 1;
      }
      if (a.vendor === OTHER_VENDOR) return 1;
      if (b.vendor === OTHER_VENDOR) return -1;
      if (a.entries.length !== b.entries.length) return b.entries.length - a.entries.length;
      return a.vendor.localeCompare(b.vendor);
    });
}

/** Stable sort: equal rows keep the catalog's own recommendation order. */
function sortEntries(entries: ModelEntry[], key: SortKey, dir: SortDir): ModelEntry[] {
  const column = SORT_COLUMNS[key];
  return entries
    .map((entry, index) => ({ entry, index }))
    .sort((x, y) => {
      const xMissing = column.missing(x.entry);
      const yMissing = column.missing(y.entry);
      if (xMissing !== yMissing) return xMissing ? 1 : -1;
      const cmp = column.compare(x.entry, y.entry);
      if (cmp !== 0) return dir === "asc" ? cmp : -cmp;
      return x.index - y.index;
    })
    .map((d) => d.entry);
}

// ---------------------------------------------------------------------------------------------
// Installs
// ---------------------------------------------------------------------------------------------

/**
 * The line under the bar. A 640 MB model arrives as several files, so "file 2 of 5" is the part
 * that tells you how much is actually left; the percentage alone would keep resetting to 0.
 */
function progressLine(p: InstallProgress): string {
  if (p.finishing) return "Finishing up…";
  const parts: string[] = [];
  if (p.fileIndex !== null && p.fileCount !== null && p.fileCount > 0) {
    parts.push(`File ${Math.min(p.fileIndex + 1, p.fileCount)} of ${p.fileCount}`);
  }
  parts.push(
    p.total > 0
      ? `${installPercent(p)}% — ${formatBytes(p.received)} of ${formatBytes(p.total)}`
      : "Starting…",
  );
  if (p.verified > 0) {
    parts.push(
      p.fileCount !== null && p.fileCount > 0
        ? `${p.verified} of ${p.fileCount} verified`
        : `${p.verified} verified`,
    );
  }
  return parts.join(" · ");
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
  /**
   * The download, read from the module-level store rather than held here: a 989 MB fetch keeps
   * going when this panel unmounts on a tab change, so its progress — and the Cancel button that
   * needs the in-flight id — must outlive the component too.
   */
  const [install, setInstall] = useState(getInstall);
  const progress = install.progress;
  const installError = install.error;
  const installNotice = install.notice;
  const cancelling = install.cancelling;
  /** Ids whose detail is expanded. Any number of rows may be open at once. */
  const [openIds, setOpenIds] = useState<ReadonlySet<string>>(() => new Set<string>());
  /** Null means the catalog's own order, which is its recommendation ranking. */
  const [sort, setSort] = useState<{ key: SortKey; dir: SortDir } | null>(null);

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

  const entries = useMemo(() => {
    if (catalog === null) return [];
    if (sort === null) return catalog.entries;
    return sortEntries(catalog.entries, sort.key, sort.dir);
  }, [catalog, sort]);

  // Sorting ranks the whole catalog against one column, which a per-maker split would silently
  // undo — so grouping applies to the catalog's own order only, and the legend says when it is
  // set aside.
  const groups = useMemo(
    () =>
      catalog === null || sort !== null ? [] : groupByVendor(catalog.entries, catalog.recommended),
    [catalog, sort],
  );

  // Mounting only *reads* the download; it never owns it. The settled callback is what makes a
  // download that finished while the user was on another tab show up as installed on return.
  useEffect(() => subscribeInstall(setInstall), []);
  useEffect(() => onInstallSettled(() => void refresh()), [refresh]);

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

  const toggleRow = (id: string) => {
    setOpenIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
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

  const allOpen = entries.length > 0 && entries.every((e) => openIds.has(e.id));

  /**
   * A header cell that sorts. Three states, cycling: default direction, reversed, then back to
   * the catalog's own order — so there is always a way back to the recommendation ranking.
   */
  const sortHeader = (key: SortKey, label: string, note: string | null) => {
    const column = SORT_COLUMNS[key];
    const active = sort !== null && sort.key === key;
    const arrow = sort !== null && sort.key === key ? (sort.dir === "asc" ? "▲" : "▼") : "";
    return (
      <>
        <button
          type="button"
          className={active ? "th-sort active" : "th-sort"}
          aria-pressed={active}
          title={`Sort by ${column.title} — click again to reverse, once more for catalog order`}
          onClick={() =>
            setSort((prev) => {
              if (prev === null || prev.key !== key) return { key, dir: column.defaultDir };
              if (prev.dir === column.defaultDir) {
                return { key, dir: column.defaultDir === "asc" ? "desc" : "asc" };
              }
              return null;
            })
          }
        >
          {label}
          <span className="th-arrow" aria-hidden="true">
            {arrow}
          </span>
        </button>
        {note !== null && <span className="th-note">{note}</span>}
      </>
    );
  };

  const renderRow = (entry: ModelEntry) => {
    const isRecommended = catalog.recommended === entry.id;
    const isSelected = selectedId === entry.id;
    const state = INSTALL_STATES[entry.install_state];
    const installing = progress !== null && progress.id === entry.id;
    const otherInstalling = progress !== null && progress.id !== entry.id;
    const canDownload =
      entry.runnable && entry.install_state !== "unpinned" && entry.install_state !== "installed";
    const canSelect = entry.runnable && entry.install_state === "installed" && !isSelected;
    const error = installError !== null && installError.id === entry.id ? installError.message : null;
    const notice =
      installNotice !== null && installNotice.id === entry.id ? installNotice.message : null;
    const open = openIds.has(entry.id);
    const detailId = `model-detail-${entry.id}`;
    const family = familyLabel(entry.engine);
    const reference = entry.measured_reference;
    const accuracy = measuredAccuracy(entry);
    // What the "Measured elsewhere" block shows: the matching target when there is one, else any
    // real measurement the catalog holds, labelled with the hardware it came from.
    const measuredPoint: MeasuredPoint | null =
      reference !== null
        ? reference
        : accuracy !== null
          ? accuracy.point
          : entry.measurements.length > 0
            ? entry.measurements[0]
            : null;
    const werEstimates = Object.entries(entry.wer_estimates);
    // The compact `~0.0145` in the column is only honest because the header says "estimates" and
    // this title spells it out in full, naming the hardware the estimate was computed for.
    const rtfTitle =
      entry.best_hardware === null
        ? formatEstimatedRtf(entry.estimated_rtf)
        : `${formatEstimatedRtf(entry.estimated_rtf)} for ${entry.best_hardware} on this machine`;

    const rowClass = ["mrow"];
    if (isRecommended) rowClass.push("recommended");
    if (isSelected) rowClass.push("selected");
    if (!entry.runnable) rowClass.push("blocked");
    if (open) rowClass.push("open");

    return (
      <li key={entry.id} className={rowClass.join(" ")}>
        <button
          type="button"
          className="mrow-main"
          aria-expanded={open}
          aria-controls={open ? detailId : undefined}
          onClick={() => toggleRow(entry.id)}
        >
          <span className="mcell mcell-arrow" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>

          <span className="mcell mcell-name">
            <span className="mname" title={entry.name}>
              {entry.name}
            </span>
            {isRecommended && (
              <span
                className="badge rec"
                title="Recommended — best fit for the machine described above"
              >
                <span aria-hidden="true">★</span>
                <span className="badge-text"> Recommended</span>
              </span>
            )}
            {!entry.runnable && (
              <span
                className="badge no"
                title={
                  entry.blockers.length > 0
                    ? `Cannot run here: ${entry.blockers.join("; ")}`
                    : "Cannot run here — open the row for the reasons"
                }
              >
                <span aria-hidden="true">✕</span>
                <span className="badge-text"> cannot run</span>
              </span>
            )}
            {/* Stands in for the family column once the window is too narrow to keep it. */}
            <span className="mname-family sub" title={entry.engine}>
              {family}
            </span>
          </span>

          <span className="mcell mcell-family" title={entry.engine}>
            <span className="vh">Family </span>
            {family}
          </span>

          <span className="mcell mcell-size num">
            <span className="vh">Download </span>
            {formatBytes(entry.download_bytes)}
          </span>

          <span className="mcell mcell-langs" title={entry.language_summary}>
            <span className="vh">Languages </span>
            {compactLanguages(entry)}
          </span>

          <span className="mcell mcell-rtf num" title={rtfTitle}>
            <span className="vh">Estimated RTF </span>
            <span className="mono estimate">{formatEstimatedRtfCompact(entry.estimated_rtf)}</span>
            {entry.best_hardware !== null && (
              <span className="hw-chip">{entry.best_hardware}</span>
            )}
          </span>

          <span className="mcell mcell-acc">
            <span
              className="tier"
              title="Editorial ranking of the model family, not a measurement"
            >
              <span className="vh">Quality tier </span>
              {entry.quality_label}
            </span>
            {accuracy !== null && (
              <span
                className="mono acc-wer"
                title={
                  accuracy.onBestHardware
                    ? `Measured: WER ${formatWer(accuracy.wer)} on ${accuracy.point.hardware} — ${accuracy.point.machine} (${accuracy.point.source})`
                    : `Measured on ${accuracy.point.hardware}, not the ${entry.best_hardware ?? "target"} path this machine would use: WER ${formatWer(accuracy.wer)} — ${accuracy.point.machine} (${accuracy.point.source})`
                }
              >
                <span className="vh">measured </span>
                {/* Naming the hardware is not decoration: a WER taken on another target must not
                    read as one taken on the path this machine would use. */}
                {accuracy.onBestHardware
                  ? `WER ${formatWer(accuracy.wer)}`
                  : `${shortHardware(accuracy.point.hardware)}: ${formatWer(accuracy.wer)}`}
              </span>
            )}
          </span>

          <span className="mcell mcell-state">
            {isSelected ? (
              <span
                className="badge sel"
                title={`In use — this is the model in Settings. ${state.label}: ${state.hint}`}
              >
                <span className="vh">Status: </span>In use
              </span>
            ) : (
              <span className={state.tone} title={`${state.label} — ${state.hint}`}>
                <span className="vh">Status: </span>
                {state.short}
              </span>
            )}
          </span>
        </button>

        {/*
          Outside the toggle, so collapsing a row can never hide a download in flight, its cancel
          button, or the outcome of one.
        */}
        {(installing || error !== null || notice !== null) && (
          <div className="mrow-live">
            {installing && progress !== null && (
              <div className="install-progress">
                <div className="progress-head">
                  <div className="progress-bar">
                    <div className="progress-fill" style={{ width: `${installPercent(progress)}%` }} />
                  </div>
                  <button
                    className="btn secondary small"
                    onClick={() => void stopInstall(entry.id)}
                    disabled={cancelling || progress.finishing}
                  >
                    {cancelling ? "Cancelling…" : "Cancel download"}
                  </button>
                </div>
                <div className="sub progress-line">{progressLine(progress)}</div>
                {progress.file !== null && <div className="sub mono path">{progress.file}</div>}
              </div>
            )}
            {error !== null && <p className="status-err">Download failed: {error}</p>}
            {notice !== null && <p className="sub">{notice}</p>}
          </div>
        )}

        {open && (
          <div className="mrow-detail" id={detailId}>
            <p className="sub model-desc">{entry.description}</p>

            {entry.install_state !== "installed" && (
              <p className="sub install-hint">
                {state.label} — {state.hint}
              </p>
            )}

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
                {measuredPoint === null ? (
                  <span className="sub">No measurement in the catalog for this model.</span>
                ) : (
                  <>
                    <MeasuredLine point={measuredPoint} />
                    {reference === null && (
                      <span className="sub">
                        Taken on {measuredPoint.hardware}, not the{" "}
                        {entry.best_hardware ?? "target"} path this machine would use — it says
                        nothing about the speed of that path.
                      </span>
                    )}
                  </>
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
                </div>
              </details>
            )}

            {werEstimates.length > 0 && (
              <p className="sub">
                WER estimates by language (published or small-sample, not a promise):{" "}
                {werEstimates.map(([lang, wer]) => `${lang} ${formatWer(wer)}`).join(", ")}
              </p>
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
            {entry.install_dir !== null && <p className="sub mono path">{entry.install_dir}</p>}
            {entry.upstream_url !== null && (
              <p className="sub mono path">Upstream: {entry.upstream_url}</p>
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
          </div>
        )}
      </li>
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
        {catalog.entries.length > 0 && (
          <button
            className="btn secondary small"
            onClick={() =>
              setOpenIds(allOpen ? new Set<string>() : new Set(entries.map((e) => e.id)))
            }
          >
            {allOpen ? "Collapse all" : "Expand all"}
          </button>
        )}
        {sort !== null && (
          <button className="btn secondary small" onClick={() => setSort(null)}>
            Catalog order
          </button>
        )}
        <span className="sub hint mono">{catalog.models_root}</span>
      </div>

      {loadError !== null && <p className="status-err">Refresh failed: {loadError}</p>}
      {selectError !== null && <p className="status-err">Could not save the model choice: {selectError}</p>}

      <p className="sub disclaimer">{catalog.estimate_disclaimer}</p>

      {catalog.entries.length === 0 ? (
        <p className="hint">The catalog is empty — no models to show.</p>
      ) : (
        <div className="model-table">
          <div className="model-thead">
            <span className="mcell mcell-arrow" />
            <span className="mcell mcell-name">{sortHeader("name", "Model", null)}</span>
            <span className="mcell mcell-family">{sortHeader("family", "Family", null)}</span>
            <span className="mcell mcell-size num">{sortHeader("size", "Download", null)}</span>
            <span className="mcell mcell-langs">{sortHeader("languages", "Languages", null)}</span>
            <span className="mcell mcell-rtf num">
              {sortHeader("speed", "Speed", "estimated RTF")}
            </span>
            <span className="mcell mcell-acc">
              {sortHeader("accuracy", "Accuracy", "tier · measured WER")}
            </span>
            <span className="mcell mcell-state">
              {sortHeader("state", "Status", "on disk")}
            </span>
          </div>

          <ul className="model-rows">
            {sort === null
              ? groups.flatMap((group) => [
                  <li className="mgroup" key={`vendor-${group.vendor}`}>
                    <h3 className="mgroup-name">{group.vendor}</h3>
                    <span className="sub mgroup-count">
                      {group.entries.length} model{group.entries.length === 1 ? "" : "s"}
                    </span>
                  </li>,
                  ...group.entries.map(renderRow),
                ])
              : entries.map(renderRow)}
          </ul>

          <p className="sub model-legend">
            {sort === null
              ? "Models are grouped by who made them — the maker of the recommended model first, then the makers offering the most, with anything uncredited under Other. Inside a group the catalog's own recommendation order is kept."
              : "Sorting ranks every model against one column, so the grouping by maker is set aside while it is on; press “Catalog order” to bring it back."}{" "}
            Click a column heading to sort, and again to reverse. <b>Speed</b> is an estimate computed from the model's speed tier and
            this machine — never a measurement; the <code>~</code> marks it and hovering gives the
            full wording and the hardware it assumes. Under <b>Accuracy</b>, the tier is an
            editorial ranking of the model family, while a green WER is a real measurement — hover
            it for the machine it was taken on, and note the chip beside it when the measurement
            was taken on hardware other than the path this machine would use. Open a row for the
            full detail and the buttons.
          </p>
        </div>
      )}
    </div>
  );
}
