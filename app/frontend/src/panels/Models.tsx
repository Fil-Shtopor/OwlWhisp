import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  deleteModel,
  getCapabilities,
  getLocalMeasurements,
  getSettings,
  listModels,
  setSettings,
  type Capabilities,
  type InstallState,
  type LocalMeasurement,
  type UnitScore,
  type LocalMeasurements,
  type ModelAccelerator,
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
  const langs = entry.language_names;
  if (langs.length === 0) return "—";
  if (langs.length <= 2) return langs.join(", ");
  return `${langs.length} languages`;
}

/**
 * "I need X, in language Y — which one?", answered from the catalog.
 *
 * The winner for a role is simply the first entry in **catalog order** that carries that role and
 * claims the chosen language. Catalog order is already the ranking the backend computed for this
 * machine (runnable first, then quality, then estimated speed), so reusing it keeps one ranking in
 * the product instead of inventing a second one here that could disagree with the recommendation
 * badge two rows below.
 *
 * A role with no winner says so rather than falling back to something that does not fill it.
 */
function RolePicks({
  catalog,
  onPick,
}: {
  catalog: ModelCatalog;
  onPick: (id: string) => void;
}) {
  const [language, setLanguage] = useState<string>("");

  // Sorted by name, not by code: the list is read alphabetically by a human looking for theirs,
  // and by code "Ukrainian" sits under `uk`, between `tt` and `ur`.
  const languages = useMemo(() => {
    const seen = new Map<string, string>();
    for (const e of catalog.entries) {
      e.languages.forEach((code, i) => seen.set(code, e.language_names[i] ?? code));
    }
    return [...seen.entries()]
      .map(([code, name]) => ({ code, name }))
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [catalog.entries]);

  const eligible = useMemo(
    () =>
      language === ""
        ? catalog.entries
        : catalog.entries.filter((e) => e.languages.includes(language)),
    [catalog.entries, language],
  );

  if (catalog.roles.length === 0) return null;

  return (
    <section className="picks">
      <div className="picks-head">
        <div className="picks-title">
          <h3>Which one should you use?</h3>
          {/*
            Said outright because the block looks like a control and is not one. Nothing here
            selects a model: the names are shortcuts to the rows below, and choosing a model is
            still Use this model in a row, or the Model setting.
          */}
          <p className="sub">
            A suggestion, not a switch — nothing here changes which model runs. Clicking a name
            opens its row.
          </p>
        </div>
        <label className="picks-lang">
          <span className="sub">Language</span>
          <select value={language} onChange={(e) => setLanguage(e.target.value)}>
            <option value="">Any</option>
            {languages.map((l) => (
              <option key={l.code} value={l.code}>
                {l.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      <ul className="picks-list">
        {catalog.roles.map((role) => {
          const winner = eligible.find((e) => e.roles.some((r) => r.id === role.id)) ?? null;
          return (
            <li key={role.id} className="pick">
              <span className={`badge role role-${role.id}`}>{role.label}</span>
              <span className="pick-blurb sub">{role.blurb}</span>
              {winner === null ? (
                <span className="pick-none sub">
                  {language === ""
                    ? "Nothing in this catalog fills that role."
                    : `Nothing here fills that role for ${
                        languages.find((l) => l.code === language)?.name ?? language
                      }.`}
                </span>
              ) : (
                <button className="pick-name" type="button" onClick={() => onPick(winner.id)}>
                  {winner.name}
                  {!winner.runnable && <span className="badge no"> cannot run here</span>}
                </button>
              )}
            </li>
          );
        })}
      </ul>
      <p className="sub picks-note">
        These are editorial roles, not measurements — the row below each name carries the numbers.
        The suggestion is the highest-ranked model for this machine that carries the role and
        claims the language.
      </p>
    </section>
  );
}

/**
 * What WER and RTF mean, and — the question this block exists to answer — where the numbers in the
 * Accuracy column come from.
 *
 * There are three plausible answers and only one is true, so it is said outright rather than left
 * to a tooltip: they are **not** the model publisher's published figures, and they are **not**
 * measured live on the machine reading this page. They were measured by this project with
 * `lw bench` on the machine named here, and committed to the catalog.
 *
 * The block names the measuring machine outright rather than calling it "yours" or "not yours".
 * Whoever reads this is on some other computer than the one the catalog was measured on, and
 * second-person wording made a fact about a specific machine read as a claim about theirs.
 *
 * It also leaves the comparison to the reader. An earlier version tried
 * to decide it — `measurement.machine === catalog.machine` — and got it backwards on the very
 * machine the numbers came from: the catalog's `machine` is the capability summary
 * ("Snapdragon(R) X2 Elite Extreme - X2E94100 - … · 18 cores · NPU Hexagon V81 via QNN EP") while
 * a measurement's is free text written when it was recorded ("ASUS Zenbook A16 - Snapdragon X2
 * Elite Extreme X2E94100, 48 GB, Windows 11 build 28000 ARM64"). Same machine, different strings,
 * and the page confidently said "that is not this machine". Naming both and saying "where they
 * differ" is right whatever the strings look like.
 */
function Glossary({ machine, entries }: { machine: string; entries: readonly ModelEntry[] }) {
  const machines = new Set<string>();
  for (const e of entries) for (const m of e.measurements) machines.add(m.machine);
  const measured = [...machines];

  return (
    <details className="glossary">
      <summary>What the numbers mean, and where they come from</summary>
      <div className="glossary-body">
        <dl className="glossary-list">
          <div className="glossary-item">
            <dt>
              WER <span className="sub">word error rate</span>
            </dt>
            <dd>
              The share of words the model got wrong — substituted, dropped or invented — against a
              reference transcript. <strong>Lower is better</strong>; 5% means about one word in
              twenty. Casing and punctuation are stripped before scoring, so neither counts as an
              error.
            </dd>
          </div>
          <div className="glossary-item">
            <dt>
              RTF <span className="sub">real-time factor</span>
            </dt>
            <dd>
              Seconds of computing per second of audio. <strong>Lower is faster</strong>: 0.05 means
              a 10-second sentence takes about half a second to transcribe, and 1.0 means it takes
              as long as it took to say.
            </dd>
          </div>
          <div className="glossary-item">
            <dt>Where the WER comes from</dt>
            <dd>
              {measured.length === 0 ? (
                "No entry in this catalog carries a measurement yet."
              ) : (
                <>
                  They are not the model publisher's published figures. Every WER in the table was
                  produced with <code>lw bench</code> over the same twelve committed speech
                  fixtures, on the CPU, on this machine:{" "}
                  {measured.map((m, i) => (
                    <span key={m}>
                      {i > 0 ? "; " : ""}
                      <span className="mono">{m}</span>
                    </span>
                  ))}
                  . The computer running this app reports itself as{" "}
                  <span className="mono">{machine}</span>.{" "}
                  <strong>
                    Where the two differ, the figures are indicative rather than a description of
                    what this computer will do
                  </strong>{" "}
                  — a real number from different silicon is still a number about different silicon.
                  The CPU, not an NPU or GPU, because the CPU is the only accelerator every model
                  here can use. <b>Benchmark</b> measures the computer running this app, and fills
                  in the <b>On your machine</b> column.
                </>
              )}
            </dd>
          </div>
          <div className="glossary-item">
            <dt>Estimated vs measured</dt>
            <dd>
              The <b>Speed</b> column is an <em>estimate</em> — arithmetic on the model's speed tier
              and your detected hardware, marked with <code>~</code>, never a measurement. A green
              WER is a measurement. The quality pill beside it (<code>good</code>,{" "}
              <code>better</code>, <code>best</code>) is an editorial ranking of the model family,
              not either of those.
            </dd>
          </div>
        </dl>
      </div>
    </details>
  );
}

/**
 * The local measurement worth putting in the row, out of however many accelerators were tried.
 *
 * Prefers the accelerator this machine would actually use for the model, because that is the run
 * that predicts what the user will experience. Falls back to whichever is fastest, so a row is
 * never blank when something was measured.
 */
function pickLocal(mine: readonly LocalMeasurement[], best: string | null): LocalMeasurement | null {
  if (mine.length === 0) return null;
  if (best !== null) {
    // `best_hardware` is a hardware-target label ("qnn_npu", "cpu"); accelerator ids use hyphens.
    const want = best.replace(/_/g, "-").toLowerCase();
    const hit = mine.find((m) => m.accelerator.toLowerCase() === want);
    if (hit !== undefined) return hit;
  }
  return [...mine].sort((a, b) => (a.warm_rtf ?? a.cold_rtf) - (b.warm_rtf ?? b.cold_rtf))[0] ?? null;
}

/** `"WER"` for a word rate, `"CER"` for a character one. */
function unitLabel(unit: UnitScore["unit"]): string {
  return unit === "character" ? "CER" : "WER";
}

/**
 * The one error rate a single-line cell should show, and whatever else was measured beside it.
 *
 * `wer` is null for every run that spanned words and characters -- which is every model claiming
 * Chinese alongside a space-delimited language -- and the cell printed a dash for those, reading
 * as "the benchmark produced nothing". It produced two things. The word rate goes in the cell,
 * because that is the unit the catalog's Accuracy column uses and the two sit side by side; the
 * character rate is named next to it, not dropped.
 */
function headlineRate(
  m: LocalMeasurement,
): { rate: number; unit: UnitScore["unit"] | null; rest: UnitScore[] } | null {
  if (m.by_unit.length > 0) {
    const primary = m.by_unit.find((u) => u.unit === "word") ?? m.by_unit[0];
    return { rate: primary.rate, unit: primary.unit, rest: m.by_unit.filter((u) => u !== primary) };
  }
  // A record written before the breakdown was stored. Its single figure is all there is, and
  // nothing says which unit it was in, so nothing claims one.
  if (m.wer !== null) return { rate: m.wer, unit: null, rest: [] };
  return null;
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

/** Accelerator ids are spelled with either separator depending on which enum produced them. */
function sameAccel(a: string, b: string): boolean {
  return a.replace(/_/g, "-").toLowerCase() === b.replace(/_/g, "-").toLowerCase();
}

/**
 * Every accelerator this build knows about, as one table: can this model use it, what has this
 * machine measured on it, and what does the catalog hold for it.
 *
 * One table instead of chips on the row plus two blocks down here. The question a reader actually
 * arrives with is "which of these has a number behind it and which is only a claim", and that is a
 * comparison across three facts per accelerator -- a chip can carry one of them, so three chips
 * carried the least useful third and took the row's width to do it.
 *
 * The columns are deliberately not merged. "Measured here" and "the catalog" were taken on
 * different machines on different days, and a single column would invite reading one as a check on
 * the other.
 */
function AcceleratorTable({
  accelerators,
  local,
  catalog,
}: {
  accelerators: readonly ModelAccelerator[];
  local: readonly LocalMeasurement[];
  catalog: readonly MeasuredPoint[];
}) {
  const matched = new Set<number>();
  accelerators.forEach((a) => {
    catalog.forEach((m, i) => {
      if (sameAccel(m.hardware, a.id)) matched.add(i);
    });
  });
  // A catalog figure whose hardware string matches no accelerator this build knows about. Listed
  // rather than dropped: it is still a real measurement, and silently hiding it would be the one
  // thing this table exists to prevent.
  const unmatched = catalog.filter((_, i) => !matched.has(i));
  // The same for a run of our own. Records written before the sherpa engine reported an
  // accelerator id are filed under "unknown": real measurements of the CPU that match no row.
  const orphanLocal = local.filter((m) => !accelerators.some((a) => sameAccel(m.accelerator, a.id)));

  const hasNumber = (a: ModelAccelerator) =>
    local.some((m) => sameAccel(m.accelerator, a.id)) ||
    catalog.some((m) => sameAccel(m.hardware, a.id));
  // Rows are for accelerators there is something to say about: it runs, or something was measured
  // on it. The rest would be eight rows of the same sentence -- a sherpa model cannot use any of
  // the NPUs or GPUs, always for the one reason -- so they are summarised under the table instead,
  // named but not given a row each.
  const rows = accelerators.filter((a) => a.supported || hasNumber(a));
  const blocked = accelerators.filter((a) => !a.supported && !hasNumber(a));
  const byReason = new Map<string, string[]>();
  for (const a of blocked) {
    const reason = a.reason ?? "not supported";
    const list = byReason.get(reason);
    if (list === undefined) byReason.set(reason, [a.label]);
    else list.push(a.label);
  }

  return (
    <div className="accel-block">
      <div className="accel-scroll">
        <table className="diag-table accel-table">
          <thead>
            <tr>
              <th>Accelerator</th>
              <th>Runs this model</th>
              <th>Measured on this machine</th>
              <th>In the catalog</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((a) => {
              const here = local.find((m) => sameAccel(m.accelerator, a.id)) ?? null;
              const there = catalog.find((m) => sameAccel(m.hardware, a.id)) ?? null;
              return (
                <tr key={a.id} className={a.supported ? "" : "accel-off"}>
                  <td>{a.label}</td>
                  <td>
                    {a.supported ? (
                      <span className="badge yes">
                        <span aria-hidden="true">✓</span>
                        <span className="badge-text"> yes</span>
                      </span>
                    ) : (
                      <>
                        <span className="badge no">
                          <span aria-hidden="true">✕</span>
                          <span className="badge-text"> no</span>
                        </span>
                        <div className="sub">{a.reason ?? "not supported"}</div>
                      </>
                    )}
                  </td>
                  <td>
                    {here === null ? (
                      <span className="sub">{a.supported ? "not measured yet" : "—"}</span>
                    ) : (
                      <>
                        <span className="mono">
                          RTF {formatRtf(here.warm_rtf ?? here.cold_rtf)}
                          {here.by_unit.length > 0
                            ? here.by_unit.map((u) => (
                                <span key={u.unit}>
                                  {" "}
                                  · {unitLabel(u.unit)} {formatWer(u.rate)}
                                </span>
                              ))
                            : here.wer !== null && <> · WER {formatWer(here.wer)}</>}
                        </span>
                        <div className="sub">
                          {here.by_unit.length > 1
                            ? here.by_unit
                                .map((u) => `${unitLabel(u.unit)} over ${u.clips}`)
                                .join(", ")
                            : here.wer === null && here.by_unit.length === 0
                              ? "no clip could be scored"
                              : `over ${here.scored_clips} clip${here.scored_clips === 1 ? "" : "s"}`}
                          {here.scored_languages.length > 0 && (
                            <> in {here.scored_languages.join("/")}</>
                          )}
                          {here.skipped_clips > 0 && (
                            <>
                              {" "}
                              ({here.skipped_clips} skipped — not this model's languages)
                            </>
                          )}{" "}
                          · {here.measured_at.slice(0, 10)}
                        </div>
                      </>
                    )}
                  </td>
                  <td>
                    {there === null ? (
                      <span className="sub">—</span>
                    ) : (
                      <span className="mono" title={`${there.machine} — ${there.source}`}>
                        RTF {formatRtf(there.rtf)}
                        {there.wer !== null && <> · WER {formatWer(there.wer)}</>}
                      </span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {[...byReason.entries()].map(([reason, labels]) => (
        <p key={reason} className="sub accel-blocked">
          <span className="badge no">
            <span aria-hidden="true">✕</span>
            <span className="badge-text"> cannot run</span>
          </span>{" "}
          {labels.join(", ")} — {reason}.
        </p>
      ))}
      <p className="sub accel-note">
        <strong>Runs this model</strong> is the backend's own answer — the same one that decides
        which accelerators a Compare-all sweep skips, so the table cannot promise a run the
        benchmark then refuses. <strong>Measured on this machine</strong> is empty until you run a
        benchmark; <strong>in the catalog</strong> was measured on the developer's machine, and the
        two are kept in separate columns because neither checks the other.
      </p>
      {orphanLocal.length > 0 && (
        <p className="sub">
          Measured here, but filed under an accelerator this build does not recognise — older runs
          recorded the backend but not which accelerator it was:{" "}
          {orphanLocal.map((m, i) => (
            <span key={`${m.accelerator}-${i}`} className="mono">
              {i > 0 && "; "}
              {m.accelerator_label}: RTF {formatRtf(m.warm_rtf ?? m.cold_rtf)}
              {m.wer !== null && <> · WER {formatWer(m.wer)}</>}
            </span>
          ))}
          . Re-run the benchmark to file them properly.
        </p>
      )}
      {unmatched.length > 0 && (
        <p className="sub">
          Also in the catalog, on hardware this build has no accelerator for:{" "}
          {unmatched.map((m, i) => (
            <span key={`${m.hardware}-${i}`} className="mono" title={`${m.machine} — ${m.source}`}>
              {i > 0 && "; "}
              {m.hardware}: RTF {formatRtf(m.rtf)}
              {m.wer !== null && <> · WER {formatWer(m.wer)}</>}
            </span>
          ))}
        </p>
      )}
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
  /** What this machine has measured, by model id. Empty until a benchmark has been run. */
  const [mine, setMine] = useState<LocalMeasurements>({});
  /** The model whose delete confirmation is open, if any. */
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
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
  /// `removeModel` is declared before `refresh` and needs to call it; a ref breaks the cycle
  /// without making either depend on the other's identity.
  const refreshRef = useRef<(() => Promise<void>) | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  /**
   * Delete one model's files. The backend takes the catalog **id**, never a path, and refuses the
   * model dictation is set to use — so this only has to report what it is told.
   */
  const removeModel = useCallback(
    async (id: string) => {
      setDeleting(id);
      setDeleteError(null);
      try {
        await deleteModel(id);
        if (alive.current) {
          setConfirmDelete(null);
          await refreshRef.current?.();
        }
      } catch (e: unknown) {
        if (alive.current) setDeleteError(String(e));
      } finally {
        if (alive.current) setDeleting(null);
      }
    },
    [],
  );

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await listModels();
      if (alive.current) {
        setCatalog(next);
        setLoadError(null);
      }
      // Fetched alongside the catalog, and therefore re-fetched whenever this panel is opened --
      // which is how a benchmark run on the other tab shows up here without any wiring between
      // the two. A failure here is not a catalog failure: the column simply stays empty.
      try {
        const local = await getLocalMeasurements();
        if (alive.current) setMine(local);
      } catch {
        if (alive.current) setMine({});
      }
    } catch (e: unknown) {
      if (alive.current) setLoadError(String(e));
    } finally {
      if (alive.current) setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    refreshRef.current = refresh;
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
    const localAll = mine[entry.id] ?? [];
    const local = pickLocal(localAll, entry.best_hardware);
    const headline = local === null ? null : headlineRate(local);
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
    const accuracy = measuredAccuracy(entry);
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
            {entry.roles.map((role) => (
              <span key={role.id} className={`badge role role-${role.id}`} title={role.blurb}>
                {role.label}
              </span>
            ))}
            {/*
              No accelerator chips here. They cost a slice of every row's width -- worst on
              Parakeet, which supports the most -- to say the least interesting third of what a
              reader wants: what a model *can* use, with no room left for whether anything was
              ever actually measured on it. The expansion answers all of that in one table.
            */}
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
                    ? `Measured on ${accuracy.point.hardware}: WER ${formatWer(accuracy.wer)} — ${accuracy.point.machine} (${accuracy.point.source})`
                    : `Measured on ${accuracy.point.hardware}, not the ${entry.best_hardware ?? "target"} path this machine would use: WER ${formatWer(accuracy.wer)} — ${accuracy.point.machine} (${accuracy.point.source})`
                }
              >
                <span className="vh">measured </span>
                {`WER ${formatWer(accuracy.wer)}`}
                {/*
                  The accelerator is shown whether or not it matches the one this machine would
                  use, mirroring the Speed column. It used to appear only on a mismatch, so a
                  figure measured on the CPU for a model that can only use the CPU looked like a
                  figure with no hardware at all — and the whole catalog is measured on the CPU,
                  because that is the only accelerator every model here can use.
                */}
                <span className="hw-chip">{shortHardware(accuracy.point.hardware)}</span>
              </span>
            )}
          </span>

          {/*
            What THIS machine measured, as opposed to the catalog's figures from the developer's.
            Empty until the user runs a benchmark, and it says so rather than showing a dash that
            could be read as "measured, and it was nothing".
          */}
          <span className="mcell mcell-mine">
            <span className="vh">On your machine </span>
            {local === null ? (
              <span className="sub mine-empty" title="Run this model in the Benchmark tab to fill this in">
                not yet
              </span>
            ) : (
              <span
                className="mono mine-value"
                title={`Measured here on ${local.accelerator_label}: ${
                  local.by_unit.length > 0
                    ? local.by_unit
                        .map(
                          (u) =>
                            `${unitLabel(u.unit)} ${formatWer(u.rate)} over ${u.clips} clip${
                              u.clips === 1 ? "" : "s"
                            }`,
                        )
                        .join(", ")
                    : local.wer === null
                      ? local.scored_clips > 0
                        ? "This run scored clips in two units and was stored before both totals were kept, so it has no figure to show. Run the benchmark again to fill it in."
                        : "nothing could be scored"
                      : `${formatWer(local.wer)} over ${local.scored_clips} clips`
                }. Warm RTF ${
                  local.warm_rtf === null ? "—" : local.warm_rtf.toFixed(4)
                }, cold ${local.cold_rtf.toFixed(4)} — ${local.measured_at}${
                  local.by_unit.length > 1
                    ? ". Two units: Chinese is scored by character, the rest by word, and the two do not average — so there is no single figure, not a missing one."
                    : ""
                }`}
              >
                {headline === null ? (
                  /*
                    A record stored before the per-unit breakdown existed, from a run that spanned
                    two units: it kept no figure at all, and there is nothing to recover from it.
                    "Re-run" says what to do; a dash said the benchmark had produced nothing.
                  */
                  local.scored_clips > 0 ? (
                    <span className="sub">re-run</span>
                  ) : (
                    "—"
                  )
                ) : (
                  formatWer(headline.rate)
                )}
                {headline !== null && headline.rest.length > 0 && (
                  <span className="hw-chip mine-alt">
                    +{headline.rest.map((u) => unitLabel(u.unit).toLowerCase()).join("/")}
                  </span>
                )}
                <span className="sub mine-rtf">
                  {" "}
                  {(local.warm_rtf ?? local.cold_rtf).toFixed(3)}
                </span>
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
              {/*
                The expansion lists every language by name, not a count. This is the one place a
                reader can answer "is mine in here?", and for a hundred-language model the count in
                the row above is exactly the number that does not answer it. The codes stay in the
                title, because that is what --languages and the manifests take.
              */}
              <div className="lang-cell">
                <dt>Languages</dt>
                <dd className="lang-list" title={entry.languages.join(", ")}>
                  {entry.language_names.length === 0 ? "—" : entry.language_names.join(", ")}
                </dd>
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
            </div>

            {/*
              One table for the whole accelerator picture: what this model can use, what this
              machine measured on it, and what the catalog holds. The three used to be a row of
              chips, a perf block and a list, which meant reading three places to answer one
              question -- and the chips were paying for their width in every row of the list.
            */}
            <AcceleratorTable
              accelerators={entry.accelerators}
              local={localAll}
              catalog={entry.measurements}
            />

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

            {/*
              The one-line verdict, kept only when it says something the accelerator table above
              does not. For a sherpa entry it used to read "CPU only - this model ships no NPU
              artifact" directly under a table already saying which accelerators are refused and
              why - and less accurately, because such a model could not use an NPU artifact even if
              it shipped one.

              Keyed on `best_hardware`, a stable id, not on the wording of the sentence: matching
              prose that a later edit can reword is exactly what this codebase refuses to do
              elsewhere, and there is no reason to start here.
            */}
            {entry.runnable && entry.best_hardware !== "cpu" && (
              <p className="sub model-reason">{entry.reason}</p>
            )}

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
              {entry.install_state !== "missing" && entry.install_state !== "unpinned" && (
                <button
                  className="btn secondary danger"
                  onClick={() => setConfirmDelete(entry.id)}
                  disabled={isSelected || deleting !== null || installing}
                  title={
                    isSelected
                      ? "This is the model dictation uses. Choose another one first."
                      : `Remove the downloaded files and free ${formatBytes(entry.disk_bytes)}`
                  }
                >
                  {deleting === entry.id ? "Deleting…" : "Delete"}
                </button>
              )}
            </div>

            {/*
              A confirmation step, because the button sits one click away from Download and the
              action cannot be undone — the files would have to be fetched again. It names the
              model and the space it frees so the dialog answers "which one, and what do I get",
              rather than only "are you sure".
            */}
            {confirmDelete === entry.id && (
              <div className="confirm" role="alertdialog" aria-label={`Delete ${entry.name}?`}>
                <p>
                  <b>Delete {entry.name}?</b> This removes the downloaded files and frees{" "}
                  {formatBytes(entry.disk_bytes)}. It cannot be undone — getting the model back
                  means downloading it again.
                </p>
                <div className="confirm-actions">
                  <button
                    className="btn secondary danger"
                    onClick={() => void removeModel(entry.id)}
                    disabled={deleting !== null}
                  >
                    {deleting === entry.id ? "Deleting…" : "Delete it"}
                  </button>
                  <button
                    className="btn secondary"
                    onClick={() => setConfirmDelete(null)}
                    disabled={deleting !== null}
                  >
                    Keep it
                  </button>
                </div>
              </div>
            )}
            {deleteError !== null && confirmDelete === entry.id && (
              <p className="status-err">{deleteError}</p>
            )}
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

      {/*
        Refresh acts on the catalog as a whole, so it stays up here with the install directory it
        re-reads. The controls that only rearrange the list moved down to sit on top of the list --
        three cards away from the rows they affected, they read as page-level actions.
      */}
      <div className="toolbar">
        <button className="btn secondary small" onClick={() => void refresh()} disabled={refreshing}>
          {refreshing ? "Refreshing…" : "Refresh"}
        </button>
        <span className="sub hint mono">{catalog.models_root}</span>
      </div>

      {loadError !== null && <p className="status-err">Refresh failed: {loadError}</p>}
      {selectError !== null && <p className="status-err">Could not save the model choice: {selectError}</p>}

      <p className="sub disclaimer">{catalog.estimate_disclaimer}</p>

      <RolePicks catalog={catalog} onPick={(id) => setOpenIds(new Set([id]))} />

      <Glossary machine={catalog.machine} entries={catalog.entries} />

      {catalog.entries.length === 0 ? (
        <p className="hint">The catalog is empty — no models to show.</p>
      ) : (
        <div className="model-table">
          <div className="model-tools">
            <button
              className="btn secondary small"
              onClick={() =>
                setOpenIds(allOpen ? new Set<string>() : new Set(entries.map((e) => e.id)))
              }
            >
              {allOpen ? "Collapse all" : "Expand all"}
            </button>
            {sort !== null && (
              <button className="btn secondary small" onClick={() => setSort(null)}>
                Catalog order
              </button>
            )}
            <span className="sub model-count">
              {catalog.entries.length} model{catalog.entries.length === 1 ? "" : "s"}
            </span>
          </div>
          <div className="model-thead">
            <span className="mcell mcell-arrow" />
            <span className="mcell mcell-name">{sortHeader("name", "Model", null)}</span>
            <span className="mcell mcell-family">{sortHeader("family", "Family", null)}</span>
            <span className="mcell mcell-size num">{sortHeader("size", "Download", null)}</span>
            <span className="mcell mcell-langs">{sortHeader("languages", "Languages", null)}</span>
            <span className="mcell mcell-rtf num">
              {sortHeader("speed", "Speed", "estimated RTF, lower is faster")}
            </span>
            <span className="mcell mcell-acc">
              {/* "catalog", paired with the "On your machine" column, so the two provenances are
                  distinguishable at a glance rather than only in the tooltip. */}
              {sortHeader("accuracy", "Accuracy", "tier · catalog WER, lower is better")}
            </span>
            <span className="mcell mcell-mine">
              <span className="th-sort-static">On your machine</span>
              <span className="th-note">error rate · RTF from your own benchmark run</span>
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
