// The benchmark run, hoisted out of the panel that displays it.
//
// A sweep takes minutes, and App.tsx unmounts the Benchmark panel the moment another tab is
// clicked. With the run's state inside that component, switching away threw the progress list, the
// elapsed clock and eventually the result itself away while the backend carried on working — so a
// run that was still going looked cancelled, and a result that arrived while the user was
// elsewhere was simply lost.
//
// The run therefore lives here, at module scope, and the panel is a view over it. Mounting
// subscribes, unmounting unsubscribes, and neither touches the run. In particular the
// `benchmark_progress` subscription is registered here and released when the sweep ends, never
// when a component goes away.
//
// This is the same shape as `recordBenchRun` in backend.ts: one module-level value, a set of
// listeners, and a getter for whoever mounts after the fact.

import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  onBenchmarkProgress,
  runBenchmark,
  runBenchmarkAll,
  type BackendOption,
  type BackendPreference,
  type BenchmarkProgress,
  type BenchReport,
  type BenchSuite,
} from "./ipc";
import { recordBenchRun } from "./backend";

/** `""` means "leave it to Settings" for both selectors. */
export type BackendChoice = "" | BackendPreference;

export interface BenchRunState {
  /** Which action is in flight, or null when nothing is. */
  readonly activity: "single" | "sweep" | null;
  /**
   * Epoch ms at which the in-flight action started; null when idle.
   *
   * The clock is *derived* from this rather than counted up, so coming back to the tab after five
   * minutes shows five minutes — not a counter that restarted when the panel remounted.
   */
  readonly startedAt: number | null;
  readonly report: BenchReport | null;
  readonly suite: BenchSuite | null;
  readonly error: string | null;
  readonly suiteError: string | null;
  readonly progress: readonly BenchmarkProgress[];
  readonly progressError: string | null;
  /** The selectors, kept here too so returning mid-run shows what the run was launched with. */
  readonly modelChoice: string;
  readonly backendChoice: BackendChoice;
}

const INITIAL: BenchRunState = {
  activity: null,
  startedAt: null,
  report: null,
  suite: null,
  error: null,
  suiteError: null,
  progress: [],
  progressError: null,
  modelChoice: "",
  backendChoice: "",
};

let state: BenchRunState = INITIAL;
const listeners = new Set<(next: BenchRunState) => void>();
let unlisten: UnlistenFn | null = null;

function set(patch: Partial<BenchRunState>): void {
  state = { ...state, ...patch };
  for (const listener of listeners) listener(state);
}

/** The run as it stands. Use this to initialise a component that has just mounted. */
export function getBenchRun(): BenchRunState {
  return state;
}

/** Subscribe to run changes. Returns the unsubscribe function. */
export function subscribeBenchRun(listener: (next: BenchRunState) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function setModelChoice(modelChoice: string): void {
  set({ modelChoice });
}

export function setBackendChoice(backendChoice: BackendChoice): void {
  set({ backendChoice });
}

function stopListening(): void {
  const fn = unlisten;
  unlisten = null;
  if (fn !== null) void fn();
}

/**
 * Register the per-backend progress subscription.
 *
 * Deliberately before the command is invoked, so the first backend's "running" event cannot be
 * missed. A failure here is not fatal — the sweep still runs, it just cannot narrate itself — so
 * it is recorded and the run continues.
 */
async function listenForProgress(): Promise<void> {
  try {
    unlisten = await onBenchmarkProgress((entry) => {
      const next = state.progress.filter((p) => p.accelerator !== entry.accelerator);
      next.push(entry);
      next.sort((a, b) => a.index - b.index);
      set({ progress: next });
    });
  } catch (e: unknown) {
    set({ progressError: String(e) });
  }
}

/** Measure one backend. Ignored while another action is in flight. */
export async function startBenchmark(): Promise<void> {
  if (state.activity !== null) return;
  const { modelChoice, backendChoice } = state;
  // One current result on screen: a sweep from five minutes ago sitting under a fresh single run
  // would read as part of it.
  set({
    activity: "single",
    startedAt: Date.now(),
    error: null,
    report: null,
    suite: null,
    suiteError: null,
    progress: [],
    progressError: null,
  });
  try {
    const result = await runBenchmark(
      modelChoice === "" ? undefined : modelChoice,
      backendChoice === "" ? undefined : backendChoice,
    );
    // File it for the Dictate panel: this run is the only proof of what actually executed.
    recordBenchRun(result, backendChoice === "" ? null : backendChoice);
    set({ report: result });
  } catch (e: unknown) {
    set({ error: String(e) });
  } finally {
    set({ activity: null, startedAt: null });
  }
}

/**
 * Measure every usable accelerator on one shared clip set.
 *
 * `backends` is only needed to name the preference that pinned the final run, so nothing is
 * attributed to a choice the user never made.
 */
export async function startBenchmarkAll(backends: readonly BackendOption[]): Promise<void> {
  if (state.activity !== null) return;
  const { modelChoice } = state;
  set({
    activity: "sweep",
    startedAt: Date.now(),
    suite: null,
    suiteError: null,
    progress: [],
    progressError: null,
    report: null,
    error: null,
  });
  try {
    await listenForProgress();
    const result = await runBenchmarkAll(modelChoice === "" ? undefined : modelChoice);
    set({ suite: result });
    // The sweep's last run is the last engine the worker held, and Dictate and Settings ask what
    // actually ran.
    const last = result.runs.length === 0 ? undefined : result.runs[result.runs.length - 1];
    if (last !== undefined) {
      const pinned =
        backends.find((o) => o.accelerator !== null && o.accelerator === last.accelerator) ?? null;
      if (pinned !== null) recordBenchRun(last, pinned.value);
    }
  } catch (e: unknown) {
    set({ suiteError: String(e) });
  } finally {
    stopListening();
    set({ activity: null, startedAt: null });
  }
}
