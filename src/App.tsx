import { useCallback, useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import logo from "./assets/logo.svg";
import { api, formatBytes, type AppView, type LauncherInfo, type Notice, type Progress, type Settings } from "./api";

export default function App() {
  const [apps, setApps] = useState<AppView[] | null>(null);
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [info, setInfo] = useState<LauncherInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [checking, setChecking] = useState(false);

  const reload = useCallback(async () => {
    try {
      setApps(await api.listApps());
      setInfo(await api.launcherInfo());
      const n = await api.takeNotice();
      if (n) setNotice(n);
    } catch (e) {
      setError(String(e));
      setApps((prev) => prev ?? []);
    }
  }, []);

  useEffect(() => {
    reload();
    api.getSettings().then(setSettings);
    const unlisten = [
      listen("apps-changed", () => reload()),
      listen<Progress>("progress", (e) => setProgress((p) => ({ ...p, [e.payload.id]: e.payload }))),
      listen<string>("error", (e) => setError(e.payload)),
      listen<string>("launcher-update-ready", () => api.launcherInfo().then(setInfo)),
    ];
    return () => unlisten.forEach((u) => u.then((f) => f()));
  }, [reload]);

  const run = (fn: () => Promise<unknown>) => async () => {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      reload();
    }
  };

  const checkNow = run(async () => {
    setChecking(true);
    try {
      await api.checkForUpdates();
    } finally {
      setChecking(false);
    }
  });

  const saveSettings = async (next: Settings) => {
    setSettings(next);
    try {
      await api.setSettings(next);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="shell">
      <header className="top">
        <div className="brand">
          <img className="mark" src={logo} alt="" width={40} height={40} />
          <div>
            <h1>Yegobox</h1>
            <p>Your apps, always up to date</p>
          </div>
        </div>
        <div className="actions">
          <button className="ghost" onClick={checkNow} disabled={checking}>
            {checking ? "Checking…" : "Check for updates"}
          </button>
          <button className="ghost" onClick={() => setShowSettings((s) => !s)} aria-expanded={showSettings}>
            Settings
          </button>
        </div>
      </header>

      {info?.updateReady && (
        <div className="banner info">
          <span>Launcher {info.updateReady} is ready.</span>
          <button onClick={() => api.restart()}>Restart to update</button>
        </div>
      )}

      {notice && (
        <div className="banner warn">
          <span>
            {notice.name} {notice.version} closed right after updating. Go back to {notice.previous}?
          </span>
          <div className="row">
            <button onClick={run(() => api.rollback(notice.appId).then(() => setNotice(null)))}>Roll back</button>
            <button className="ghost" onClick={() => setNotice(null)}>
              Keep {notice.version}
            </button>
          </div>
        </div>
      )}

      {error && (
        <div className="banner error" role="alert">
          <span>{error}</span>
          <button className="ghost" onClick={() => setError(null)}>
            Dismiss
          </button>
        </div>
      )}

      {showSettings && settings && <SettingsPanel settings={settings} onChange={saveSettings} />}

      <main className="grid">
        {apps === null && <p className="muted">Loading apps…</p>}
        {apps?.length === 0 && !error && <p className="muted">No apps in the catalog yet.</p>}
        {apps?.map((app) => (
          <AppCard
            key={app.id}
            app={app}
            progress={progress[app.id]}
            launcherUpdateReady={!!info?.updateReady}
            onInstall={run(() => api.install(app.id))}
            onOpen={run(() => api.open(app.id))}
            onRestart={() => api.restart()}
          />
        ))}
      </main>

      <footer className="foot muted">
        Launcher {info?.version ?? ""}
        {info?.platform ? ` · ${info.platform}` : ""}
        {settings?.channel === "beta" ? " · beta channel" : ""}
      </footer>
    </div>
  );
}

function AppCard(props: {
  app: AppView;
  progress?: Progress;
  launcherUpdateReady: boolean;
  onInstall: () => void;
  onOpen: () => void;
  onRestart: () => void;
}) {
  const { app, progress } = props;
  const pct = progress?.total ? Math.round((progress.done / progress.total) * 100) : null;

  let detail: string;
  let primary: ReactNode;
  switch (app.status) {
    case "unsupported":
      detail = "Not available for this computer yet";
      primary = <button disabled>Unavailable</button>;
      break;
    case "not_installed":
      detail = [app.available, formatBytes(app.size)].filter(Boolean).join(" · ");
      primary = <button onClick={props.onInstall}>Install</button>;
      break;
    case "installing":
      detail = pct === null ? "Downloading…" : pct >= 100 ? "Installing…" : `Downloading… ${pct}%`;
      primary = <button disabled>{app.installed ? "Updating" : "Installing"}</button>;
      break;
    case "needs_launcher_update":
      detail = `${app.available} needs a newer launcher`;
      primary = props.launcherUpdateReady ? (
        <button onClick={props.onRestart}>Restart launcher</button>
      ) : (
        <button disabled>Updating launcher…</button>
      );
      break;
    case "update_available":
      detail = `${app.installed} → ${app.available}`;
      primary = <button onClick={props.onInstall}>Update</button>;
      break;
    case "update_staged":
      detail = app.running
        ? `${app.staged} is ready. Close ${app.name} to finish updating.`
        : `${app.staged} is ready and installs when you open it`;
      primary = <button onClick={props.onOpen}>{app.running ? "Show" : "Open"}</button>;
      break;
    default:
      detail = `Version ${app.installed}`;
      primary = <button onClick={props.onOpen}>{app.running ? "Show" : "Open"}</button>;
  }

  return (
    <article className="card">
      <div className="card-head">
        {app.icon ? <img src={app.icon} alt="" width={56} height={56} /> : <span className="icon-fallback">{app.name[0]}</span>}
        <div className="card-title">
          <h2>{app.name}</h2>
          <p className="muted">{detail}</p>
        </div>
        {app.running && <span className="pill">Running</span>}
      </div>
      {app.status === "installing" && (
        <div className="bar" aria-label="progress">
          <div style={{ width: `${pct ?? 5}%` }} />
        </div>
      )}
      <div className="card-foot">
        {primary}
        {app.status === "update_available" && (
          <button className="ghost" onClick={props.onOpen}>
            Open {app.installed}
          </button>
        )}
      </div>
      {app.notes && (
        <details>
          <summary>What's new in {app.available}</summary>
          <p className="notes">{app.notes}</p>
        </details>
      )}
    </article>
  );
}

function SettingsPanel({ settings, onChange }: { settings: Settings; onChange: (s: Settings) => void }) {
  return (
    <section className="settings">
      <label>
        Update channel
        <select
          value={settings.channel}
          onChange={(e) => onChange({ ...settings, channel: e.target.value as Settings["channel"] })}
        >
          <option value="stable">Stable</option>
          <option value="beta">Beta (early builds)</option>
        </select>
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={settings.auto_update}
          onChange={(e) => onChange({ ...settings, auto_update: e.target.checked })}
        />
        Download updates automatically
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={settings.launch_at_login}
          onChange={(e) => onChange({ ...settings, launch_at_login: e.target.checked })}
        />
        Start at login (keeps apps updated in the background)
      </label>
    </section>
  );
}
