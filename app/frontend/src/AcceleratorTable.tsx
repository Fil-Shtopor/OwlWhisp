// The "what can this machine actually use" table, shared by Settings and Diagnostics.
//
// `present`, `registered`, `devices` and `usable` are four separate columns on purpose. Collapsing
// them into one badge would hide the two failures that actually happen: a provider library that is
// installed but fails to load, and one that loads but enumerates no device. Only `usable` means
// acceleration.

import type { AcceleratorReport, AcceleratorStatus } from "./ipc";

function YesNo({ value }: { value: boolean }) {
  return <span className={value ? "badge yes" : "badge no"}>{value ? "yes" : "no"}</span>;
}

function Row({ status }: { status: AcceleratorStatus }) {
  return (
    <tbody className="accel-group">
      <tr>
        <td className="accel-name">
          <span className="accel-label">{status.label}</span>
          <span className="sub accel-library">
            {status.library === null
              ? "built into ONNX Runtime"
              : status.library}
          </span>
        </td>
        <td>
          <span className="badge">{status.kind_label}</span>
        </td>
        <td>{status.vendor === null ? <span className="sub">vendor-neutral</span> : status.vendor}</td>
        <td>
          <YesNo value={status.present} />
        </td>
        <td>
          <YesNo value={status.registered} />
        </td>
        <td className="accel-devices">{status.devices}</td>
        <td>
          <span className={status.usable ? "badge yes" : "badge no"}>
            {status.usable ? "usable" : "no"}
          </span>
        </td>
      </tr>
      <tr className="accel-detail">
        <td colSpan={7}>
          <span className="sub">{status.detail}</span>
          {/*
            The line between supporting a GPU and supporting an NPU, and it decides whether the
            user has anything to download: an NPU wants a graph quantized and compiled for that
            silicon, a GPU consumes the ordinary one.
          */}
          {status.needs_dedicated_artifact ? (
            <span className="sub">
              {" "}
              — needs a model artifact compiled for it, so a model without one cannot use it.
            </span>
          ) : status.kind === "cpu" ? null : (
            <span className="sub">
              {" "}
              — runs the ordinary model graph, so a standard install needs no extra download.
            </span>
          )}
        </td>
      </tr>
    </tbody>
  );
}

export interface AcceleratorTableProps {
  /** The probe result, or null while it has not arrived. */
  report: AcceleratorReport | null;
  /** The IPC call itself failed (not the same as `report.error`). */
  loadError: string | null;
  /** A probe is in flight. */
  loading: boolean;
}

export function AcceleratorTable({ report, loadError, loading }: AcceleratorTableProps) {
  if (loadError !== null) {
    return (
      <p className="status-err">
        Could not ask the backend what this machine has: {loadError}. Availability is unknown.
      </p>
    );
  }
  if (report === null) {
    return <p className="hint">{loading ? "Probing this machine…" : "Not probed yet."}</p>;
  }
  // A report-level error is "we could not find out", which is not a hardware verdict: showing a
  // table of "no" here would read as "this machine has nothing", which we do not know.
  if (report.error !== null) {
    return (
      <p className="status-err">
        {report.error}. Availability is unknown — that is not the same as nothing being available.
      </p>
    );
  }
  if (report.items.length === 0) {
    return (
      <p className="hint">
        The probe returned no accelerators at all, not even the CPU provider. Nothing can be said
        about this machine.
      </p>
    );
  }

  return (
    <div className="accel-table-wrap">
      <table className="diag-table accel-table">
        <thead>
          <tr>
            <th>accelerator</th>
            <th>kind</th>
            <th>vendor</th>
            <th title="Its provider library is in the runtime directory">library</th>
            <th title="Its provider loaded and registered with ONNX Runtime in this process">
              registered
            </th>
            <th title="How many matching devices it enumerated">devices</th>
            <th title="A device of its class enumerated — the only one that means acceleration">
              usable
            </th>
          </tr>
        </thead>
        {report.items.map((item) => (
          <Row key={item.id} status={item} />
        ))}
      </table>
      <p className="sub accel-legend">
        These are four separate facts. <strong>library</strong> is the provider file being installed,{" "}
        <strong>registered</strong> is ONNX Runtime having loaded it, <strong>devices</strong> is
        what it then found on this machine, and only <strong>usable</strong> means acceleration:
        a driver can be installed while the provider fails to load, and a provider can load while
        finding no device.
      </p>
    </div>
  );
}
