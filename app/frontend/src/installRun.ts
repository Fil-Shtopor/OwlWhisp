// The model download, hoisted out of the panel that displays it.
//
// Same problem and same shape as `benchRun.ts`: App.tsx unmounts the Models panel on a tab change,
// and a 989 MB download does not stop because somebody looked at Settings. Holding the progress in
// component state meant switching tabs lost the bar, the "file 3 of 5" line and — worse — the
// Cancel button, which needs the in-flight id. Coming back showed a model that was neither
// installed nor visibly downloading.
//
// Two things this has to get right beyond simply surviving:
//
//  - the terminal events must still be recorded while nobody is mounted, so returning shows
//    "installed", "failed" or "cancelled" instead of a bar frozen at 61%;
//  - the caller is told when a download finishes, so the catalog can be re-read. That is
//    `onSettled`, registered by whoever is mounted and dropped when they go — the run itself never
//    depends on a listener being there.

import {
  cancelInstall,
  installModel,
  type InstallEvent,
} from "./ipc";

/** Progress of the one install that can be in flight at a time. */
export interface InstallProgress {
  readonly id: string;
  readonly file: string | null;
  readonly received: number;
  readonly total: number;
  /** 0-based, from the `progress` event. Null until the first one arrives. */
  readonly fileIndex: number | null;
  readonly fileCount: number | null;
  readonly verified: number;
  readonly finishing: boolean;
}

export interface InstallState {
  readonly progress: InstallProgress | null;
  readonly error: { readonly id: string; readonly message: string } | null;
  readonly notice: { readonly id: string; readonly message: string } | null;
  /** True between pressing Cancel and the download actually stopping. */
  readonly cancelling: boolean;
}

const INITIAL: InstallState = { progress: null, error: null, notice: null, cancelling: false };

let state: InstallState = INITIAL;
const listeners = new Set<(next: InstallState) => void>();
/** Fires once per finished download, whatever the outcome, so the catalog can be re-read. */
const settledListeners = new Set<() => void>();

function set(patch: Partial<InstallState>): void {
  state = { ...state, ...patch };
  for (const listener of listeners) listener(state);
}

export function getInstall(): InstallState {
  return state;
}

export function subscribeInstall(listener: (next: InstallState) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Subscribe to "a download has finished" — installed, cancelled or failed alike. */
export function onInstallSettled(listener: () => void): () => void {
  settledListeners.add(listener);
  return () => {
    settledListeners.delete(listener);
  };
}

/** Apply one streamed event to the in-flight download. */
function apply(id: string, event: InstallEvent): void {
  const p = state.progress;
  const mine = p !== null && p.id === id;
  switch (event.event) {
    case "file_started":
      if (mine) set({ progress: { ...p, file: event.path, received: 0, total: event.total } });
      break;
    case "progress":
      if (mine) {
        set({
          progress: {
            ...p,
            file: event.path,
            received: event.received,
            total: event.total,
            fileIndex: event.file_index,
            fileCount: event.file_count,
          },
        });
      }
      break;
    case "file_verified":
      if (mine) set({ progress: { ...p, verified: p.verified + 1 } });
      break;
    case "completed":
      // Two of these arrive: a bare one from the downloader, then one carrying `dir`. Both mean
      // the same thing, so the later message simply replaces the earlier one.
      if (mine) set({ progress: { ...p, finishing: true } });
      set({
        notice: {
          id,
          message:
            event.dir === undefined
              ? "Downloaded and verified."
              : `Downloaded and verified into ${event.dir}`,
        },
      });
      break;
    case "cancelled":
      // Not an error: the command resolves after this, and staging keeps the partial files.
      set({
        error: null,
        notice: {
          id,
          message: "Download cancelled. Partial files stay in staging and resume next time.",
        },
      });
      break;
    case "failed":
      set({ error: { id, message: event.message } });
      break;
  }
}

/** Download and verify a model. Ignored while another download is in flight. */
export async function startInstall(id: string): Promise<void> {
  if (state.progress !== null) return;
  set({
    error: null,
    notice: null,
    cancelling: false,
    progress: {
      id,
      file: null,
      received: 0,
      total: 0,
      fileIndex: null,
      fileCount: null,
      verified: 0,
      finishing: false,
    },
  });
  try {
    await installModel(id, (e) => apply(id, e));
    // Resolved: completed or cancelled, and the matching event has almost certainly already set
    // the notice. Only fill one in if none landed — the channel and the promise are separate
    // deliveries, so their order is not guaranteed.
    if (state.notice === null || state.notice.id !== id) {
      set({ notice: { id, message: "Downloaded and verified." } });
    }
  } catch (e: unknown) {
    // A `failed` event carries the better message; only fall back to the rejection.
    if (state.error === null || state.error.id !== id) {
      set({ error: { id, message: String(e) } });
    }
  } finally {
    set({ progress: null, cancelling: false });
    for (const listener of settledListeners) listener();
  }
}

/** Ask an in-flight download to stop. Partial files stay in staging and resume later. */
export async function stopInstall(id: string): Promise<void> {
  set({ cancelling: true });
  try {
    await cancelInstall(id);
  } catch (e: unknown) {
    set({ cancelling: false, error: { id, message: String(e) } });
  }
}

/** Progress of the file currently downloading — the bar is per file, not per download. */
export function installPercent(p: InstallProgress): number {
  if (p.total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((p.received / p.total) * 100)));
}
