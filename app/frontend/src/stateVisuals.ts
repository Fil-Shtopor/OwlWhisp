// Shared visual mapping for the recording state machine.
// "loading" is a UI-only state used before the backend has reported anything.

import type { RecordingState } from "./ipc";

export type UiState = RecordingState | "loading";

export interface StateVisual {
  readonly label: string;
  readonly color: string;
  readonly pulse: boolean;
}

export const STATE_VISUALS: Record<UiState, StateVisual> = {
  loading: { label: "Loading…", color: "#f0b429", pulse: true }, // amber
  idle: { label: "Idle", color: "#6b7280", pulse: false }, // gray
  listening: { label: "Listening", color: "#ef4444", pulse: true }, // red
  processing: { label: "Processing", color: "#3b82f6", pulse: true }, // blue
  done: { label: "Done", color: "#22c55e", pulse: false }, // green
  error: { label: "Error", color: "#b91c1c", pulse: false }, // gray-red
};
