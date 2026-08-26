import { useCallback, useEffect, useState, type ReactNode } from "react";
import { getDiagnostics, type Diagnostics, type JsonValue } from "../ipc";

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

function orderedKeys(diag: Diagnostics): string[] {
  const known = PREFERRED_ORDER.filter((k) => k in diag);
  const rest = Object.keys(diag)
    .filter((k) => !PREFERRED_ORDER.includes(k))
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
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setDiag(await getDiagnostics());
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div>
      <div className="toolbar">
        <button className="btn secondary" onClick={() => void refresh()} disabled={busy}>
          {busy ? "Probing…" : "Refresh"}
        </button>
        <span className="sub hint">
          Probes the ONNX Runtime and enumerates QNN/NPU devices; may take a moment.
        </span>
      </div>
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
    </div>
  );
}
