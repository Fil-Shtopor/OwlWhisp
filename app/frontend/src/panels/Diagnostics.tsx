import { useEffect, useState, type ReactNode } from "react";
import { getDiagnostics, type AcceleratorReport, type Diagnostics, type JsonValue } from "../ipc";
import { AcceleratorTable } from "../AcceleratorTable";
import { probeAccelerators } from "../backend";

/** Order matters: the interesting NPU facts first, everything else alphabetically after. */
const PREFERRED_ORDER: readonly string[] = [
  "npu_available",
  "qnn_npu_count",
  "qnn_dll_present",
  "qnn_registered",
  "devices",
  "runtime_dir",
  "runtime_error",
  "app_version",
  "core_version",
  "os",
  "os_version",
  "arch",
];

/**
 * Keys rendered by a dedicated section above, so the generic dump does not repeat them as JSON.
 *
 * `accelerators` carries the same `{error, items}` shape as `list_accelerators`; the table reads
 * far better than a pretty-printed blob and keeps the four facts distinct.
 */
const RENDERED_SEPARATELY: ReadonlySet<string> = new Set(["accelerators"]);

function orderedKeys(diag: Diagnostics): string[] {
  const known = PREFERRED_ORDER.filter((k) => k in diag && !RENDERED_SEPARATELY.has(k));
  const rest = Object.keys(diag)
    .filter((k) => !PREFERRED_ORDER.includes(k) && !RENDERED_SEPARATELY.has(k))
    .sort();
  return [...known, ...rest];
}

function renderValue(value: JsonValue): ReactNode {
  if (typeof value === "boolean") {
    return <span className={value ? "badge yes" : "badge no"}>{value ? "yes" : "no"}</span>;
  }
  if (value === null) {
    return <span className="sub">–</span>;
  }
  if (Array.isArray(value)) {
    if (value.length === 0) return <span className="sub">none</span>;
    return (
      <ul>
        {value.map((item, i) => (
          <li key={i}>{typeof item === "string" ? item : JSON.stringify(item)}</li>
        ))}
      </ul>
    );
  }
  if (typeof value === "object") {
    return <pre style={{ margin: 0 }}>{JSON.stringify(value, null, 2)}</pre>;
  }
  return String(value);
}

export function DiagnosticsPanel() {
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [accel, setAccel] = useState<AcceleratorReport | null>(null);
  const [accelError, setAccelError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [nonce, setNonce] = useState(0);

  // One effect for both reads, keyed on a nonce so "Refresh" re-runs it with the same unmount
  // guard as the initial load.
  useEffect(() => {
    let disposed = false;
    setBusy(true);
    const diagnostics = getDiagnostics()
      .then((d) => {
        if (!disposed) {
          setDiag(d);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) setError(String(e));
      });
    // Refresh means "look again", so it forces a fresh probe rather than reusing the session one.
    const accelerators = probeAccelerators(nonce > 0)
      .then((report) => {
        if (!disposed) {
          setAccel(report);
          setAccelError(null);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) {
          setAccel(null);
          setAccelError(String(e));
        }
      });
    void Promise.allSettled([diagnostics, accelerators]).then(() => {
      if (!disposed) setBusy(false);
    });
    return () => {
      disposed = true;
    };
  }, [nonce]);

  return (
    <div className="diagnostics">
      <div className="toolbar">
        <button className="btn secondary" onClick={() => setNonce((n) => n + 1)} disabled={busy}>
          {busy ? "Probing…" : "Refresh"}
        </button>
        <span className="sub hint">
          Probes the ONNX Runtime and enumerates devices for every execution provider it can load;
          may take a moment.
        </span>
      </div>

      <section className="diag-section">
        <h3 className="bench-heading">Accelerators on this machine</h3>
        <AcceleratorTable report={accel} loadError={accelError} loading={busy} />
      </section>

      <section className="diag-section">
        <h3 className="bench-heading">Raw diagnostics</h3>
        {error !== null && <p className="status-err">Diagnostics failed: {error}</p>}
        {diag === null && error === null && <p className="hint">Collecting diagnostics…</p>}
        {diag !== null && (
          <table className="diag-table">
            <tbody>
              {orderedKeys(diag).map((key) => (
                <tr key={key}>
                  <th>{key}</th>
                  <td>{renderValue(diag[key] ?? null)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
