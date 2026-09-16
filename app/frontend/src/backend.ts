// Shared reasoning about accelerators: which one a preference would actually be served by on this
// machine, and what the UI is allowed to claim about it.
//
// House rule, in the same spirit as format.ts's estimate/measurement split: `usable` is the only
// field that means acceleration. `present`, `registered` and `devices` answer different questions
// and routinely disagree — a driver can be installed while the provider fails to load, and a
// provider can load while finding no device — so they are never collapsed into one verdict.
//
// Labels are never spelled here. They come from `BackendOption.label` and `AcceleratorStatus.label`,
// i.e. from the Rust side, so a backend this build gains cannot arrive unnamed or misnamed.

import {
  listAccelerators,
  type AcceleratorReport,
  type AcceleratorStatus,
  type ActiveBackend,
  type BackendOption,
  type BackendPreference,
  type BenchReport,
} from "./ipc";

// ---------------------------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------------------------

let pendingProbe: Promise<AcceleratorReport> | null = null;

/**
 * `list_accelerators`, shared across panels for the life of the session.
 *
 * The probe loads provider libraries and enumerates devices, so four panels each re-running it on
 * every tab switch is real work for an answer that does not change on its own. `force` is for the
 * one case where it does: the user installed a provider and asked to look again.
 *
 * A rejected probe is never cached — the next caller retries rather than inheriting the failure.
 */
export function probeAccelerators(force = false): Promise<AcceleratorReport> {
  if (force || pendingProbe === null) {
    const probe: Promise<AcceleratorReport> = listAccelerators().catch((e: unknown) => {
      if (pendingProbe === probe) pendingProbe = null;
      throw e;
    });
    pendingProbe = probe;
  }
  return pendingProbe;
}

/**
 * The preferences that name a *kind* of hardware rather than one exact provider.
 *
 * `force_cpu` is here even though it pins an accelerator: it is a coarse choice to the user, and
 * `list_backends` returns these four first for exactly that reason.
 */
export const COARSE_PREFERENCES: readonly BackendPreference[] = [
  "automatic",
  "force_npu",
  "force_gpu",
  "force_cpu",
];

export function isCoarse(option: BackendOption): boolean {
  return COARSE_PREFERENCES.includes(option.value);
}

/**
 * How far a choice has been verified on this machine.
 *
 * `unknown` is not `unusable`: it means the probe could not answer, which must never be rendered
 * as a hardware verdict.
 */
export type Availability = "usable" | "unusable" | "unsupported" | "unknown";

export interface BackendResolution {
  availability: Availability;
  /** The accelerator that would serve the choice, when one could be identified. */
  accelerator: AcceleratorStatus | null;
  /** One line, written to be shown verbatim. */
  reason: string;
}

/** The status entry for an accelerator id, or null when this platform has no such provider. */
export function findAccelerator(report: AcceleratorReport, id: string): AcceleratorStatus | null {
  return report.items.find((item) => item.id === id) ?? null;
}

function resolveKind(
  report: AcceleratorReport,
  kind: "gpu" | "npu",
  noun: string,
): BackendResolution {
  // `items` arrives in the order the automatic policy prefers, so the first usable one of the
  // right kind is the one that would be chosen.
  const chosen = report.items.find((item) => item.kind === kind && item.usable);
  if (chosen !== undefined) {
    return { availability: "usable", accelerator: chosen, reason: `Would use ${chosen.label}.` };
  }
  const anyOfKind = report.items.some((item) => item.kind === kind);
  return {
    availability: "unusable",
    accelerator: null,
    reason: anyOfKind
      ? `No ${noun} on this machine probed usable — the table below says why.`
      : `This build has no ${noun} provider for this platform.`,
  };
}

/**
 * What would serve `option` on this machine.
 *
 * `report === null` means "not probed yet", which resolves to `unknown` rather than to a verdict.
 */
export function resolveOption(
  option: BackendOption,
  report: AcceleratorReport | null,
): BackendResolution {
  if (report === null) {
    return {
      availability: "unknown",
      accelerator: null,
      reason: "Accelerator availability has not been probed yet.",
    };
  }
  if (report.error !== null) {
    return { availability: "unknown", accelerator: null, reason: report.error };
  }

  const id = option.accelerator;
  if (id !== null) {
    const status = findAccelerator(report, id);
    if (status === null) {
      return {
        availability: "unsupported",
        accelerator: null,
        reason: "Not supported on this platform — this build ships no provider for it here.",
      };
    }
    return {
      availability: status.usable ? "usable" : "unusable",
      accelerator: status,
      reason: status.detail,
    };
  }

  switch (option.value) {
    case "automatic": {
      const first = report.items.find((item) => item.usable);
      if (first === undefined) {
        return {
          availability: "unusable",
          accelerator: null,
          reason: "Nothing on this machine probed usable — not even the CPU provider.",
        };
      }
      return {
        availability: "usable",
        accelerator: first,
        reason: `Would use ${first.label}: the first usable accelerator, with the CPU last.`,
      };
    }
    case "force_npu":
      return resolveKind(report, "npu", "NPU");
    case "force_gpu":
      return resolveKind(report, "gpu", "GPU");
    default:
      // A coarse preference this build of the UI has never heard of. Say so rather than guess.
      return {
        availability: "unknown",
        accelerator: null,
        reason: "This screen does not know which accelerator this choice resolves to.",
      };
  }
}

/** The one-line consequence of picking this option, which is the part users get wrong. */
export function strictnessNote(option: BackendOption): string {
  return option.strict
    ? "Strict: if it cannot be honoured the run fails instead of quietly going somewhere else."
    : "Falls back to the CPU when nothing faster works, so a run always completes.";
}

/** The backend's own label for a preference, or the raw serde value when the list is not loaded. */
export function preferenceLabel(
  options: readonly BackendOption[],
  preference: BackendPreference,
): string {
  return options.find((o) => o.value === preference)?.label ?? preference;
}

// ---------------------------------------------------------------------------------------------
// Last measured run
// ---------------------------------------------------------------------------------------------
//
// The only source that says what *did* run, rather than what should, is a benchmark report. The
// Benchmark panel files one here when a run finishes, so Dictate can show it; nothing is invented
// and no new IPC command exists for it. It is session state: a fresh process has none, and the
// panel then says "not measured" rather than pretending.

/** What a finished benchmark proved about the backend. */
export interface LastRun {
  /** The backend that actually executed, e.g. "QNN on NPU". Straight from `BenchReport.backend`. */
  backend: string;
  /** The preference the run asked for, or null when it used the one in Settings. */
  requested: BackendPreference | null;
  /** The engine's selection notes, including any fallback. */
  notes: readonly string[];
  /** When the run finished. */
  at: number;
}

let lastRun: LastRun | null = null;
const listeners = new Set<(run: LastRun) => void>();

/** Record a finished benchmark. `requested` is what the user asked for, null for "from Settings". */
export function recordBenchRun(report: BenchReport, requested: BackendPreference | null): void {
  const run: LastRun = {
    backend: report.backend,
    requested,
    notes: report.notes,
    at: Date.now(),
  };
  lastRun = run;
  for (const listener of listeners) listener(run);
}

export function getLastRun(): LastRun | null {
  return lastRun;
}

/** Subscribe to benchmark results filed in this session. Returns the unsubscribe function. */
export function subscribeLastRun(listener: (run: LastRun) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Whether a recorded run still describes the current preference.
 *
 * A run launched with an explicit backend says nothing about a different one, so it is only
 * carried over when it ran "from Settings" or with the very preference now selected.
 */
export function appliesTo(run: LastRun, preference: BackendPreference): boolean {
  return run.requested === null || run.requested === preference;
}

// ---------------------------------------------------------------------------------------------
// The "what is running" line
// ---------------------------------------------------------------------------------------------

/**
 * The loaded engine's own answer, in the same shape as `BenchReport.backend` ("QNN on NPU"),
 * with the device appended when it named one.
 *
 * Every part is optional in the contract, so each is checked rather than assumed: a `loaded`
 * engine that named nothing still gets a truthful line instead of "null on null".
 */
export function describeActive(active: ActiveBackend): string {
  const { provider, acceleration, device } = active;
  let head: string;
  if (provider !== null && acceleration !== null) head = `${provider} on ${acceleration}`;
  else if (provider !== null) head = provider;
  else if (acceleration !== null) head = acceleration;
  else head = "an engine that did not name its provider";
  return device === null ? head : `${head} (${device})`;
}

/** How far the line can be trusted, which also decides how it is styled. */
export type BackendLineTone = "running" | "pending" | "measured" | "probed" | "warning";

export interface BackendLine {
  tone: BackendLineTone;
  /** One short line, for the Dictate panel. */
  text: string;
  /** The longer story, for a tooltip or a `sub` line. */
  detail: string;
}

/**
 * The single line that answers "what is dictation running on right now?".
 *
 * Order of authority: a measurement from this session, then a live probe of the preference, then
 * an admission that we do not know. Nothing here promises acceleration that was not probed.
 */
export function runningBackendLine(
  preference: BackendPreference | null,
  options: readonly BackendOption[],
  report: AcceleratorReport | null,
  run: LastRun | null,
): BackendLine {
  if (preference === null) {
    return {
      tone: "pending",
      text: "Checking which backend will run…",
      detail: "Reading the saved preference and probing this machine.",
    };
  }

  const label = preferenceLabel(options, preference);

  if (run !== null && appliesTo(run, preference)) {
    return {
      tone: "measured",
      text: `Running on ${run.backend} — measured by this session's benchmark.`,
      detail:
        run.notes.length === 0
          ? `Selected backend: ${label}.`
          : `Selected backend: ${label}. Engine notes: ${run.notes.join(" · ")}`,
    };
  }

  const option = options.find((o) => o.value === preference) ?? null;
  const resolution: BackendResolution =
    option === null
      ? {
          availability: "unknown",
          accelerator: null,
          reason: "The list of backends could not be read.",
        }
      : resolveOption(option, report);

  switch (resolution.availability) {
    case "usable": {
      const target = resolution.accelerator;
      const via = target === null || target.label === label ? label : `${label} → ${target.label}`;
      return {
        tone: "probed",
        text: `Backend: ${via} — probed usable, not measured yet.`,
        detail: `${resolution.reason} Run a benchmark to see what actually executes.`,
      };
    }
    case "unusable":
    case "unsupported":
      return {
        tone: "warning",
        text: `Backend: ${label} — not usable on this machine.`,
        detail: resolution.reason,
      };
    case "unknown":
      return {
        tone: "pending",
        text: `Backend: ${label} — availability not established.`,
        detail: resolution.reason,
      };
  }
}
