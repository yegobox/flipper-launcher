import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  formatBytes,
  formatDate,
  platformLabel,
  timeAgo,
  GITHUB_URL,
  RELEASES_URL,
  WEBSITE_URL,
  type AppView,
  type LauncherInfo,
  type Notice,
  type Progress,
  type Settings,
} from "./api";

/** Card accents when the catalog gives none (cycled by position). */
const ACCENTS = ["#2f6bff", "#f0553a", "#8b5cf6", "#f5b015", "#14b8a6", "#ec4899"];

type Filter = "all" | "installed" | "updates" | "not_installed";

const isInstalled = (a: AppView) => a.installed !== null;
const hasUpdate = (a: AppView) => a.status === "update_available" || a.status === "update_staged";

export default function App() {
  const [apps, setApps] = useState<AppView[] | null>(null);
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [info, setInfo] = useState<LauncherInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [checking, setChecking] = useState(false);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [now, setNow] = useState(Date.now());
  /** Android: false until the user allows Atlas to install apps. */
  const [canInstall, setCanInstall] = useState(true);
  const isAndroid = info?.platform === "android";

  const reload = useCallback(async () => {
    try {
      setApps(await api.listApps());
      const li = await api.launcherInfo();
      setInfo(li);
      if (li.platform === "android") setCanInstall(await api.androidCanInstall());
      const n = await api.takeNotice();
      if (n) setNotice(n);
    } catch (e) {
      setError(String(e));
      setApps((prev) => prev ?? []);
      api.launcherInfo().then(setInfo);
    }
  }, []);

  useEffect(() => {
    reload();
    api.getSettings().then(setSettings);
    const tick = setInterval(() => setNow(Date.now()), 30_000);
    // Android: coming back from the "Install unknown apps" setting.
    const onVisible = () => document.visibilityState === "visible" && reload();
    document.addEventListener("visibilitychange", onVisible);
    const unlisten = [
      listen("apps-changed", () => reload()),
      listen<Progress>("progress", (e) => setProgress((p) => ({ ...p, [e.payload.id]: e.payload }))),
      listen<string>("error", (e) => setError(e.payload)),
      listen<string>("launcher-update-ready", () => api.launcherInfo().then(setInfo)),
    ];
    return () => {
      clearInterval(tick);
      document.removeEventListener("visibilitychange", onVisible);
      unlisten.forEach((u) => u.then((f) => f()));
    };
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

  const counts = useMemo(() => {
    const list = apps ?? [];
    return {
      all: list.length,
      installed: list.filter(isInstalled).length,
      updates: list.filter(hasUpdate).length,
      not_installed: list.filter((a) => !isInstalled(a)).length,
    };
  }, [apps]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (apps ?? [])
      .map((app, index) => ({ app, index }))
      .filter(({ app }) => {
        if (filter === "installed" && !isInstalled(app)) return false;
        if (filter === "updates" && !hasUpdate(app)) return false;
        if (filter === "not_installed" && isInstalled(app)) return false;
        return !q || app.name.toLowerCase().includes(q) || (app.description ?? "").toLowerCase().includes(q);
      });
  }, [apps, filter, query]);

  const filters: { key: Filter; label: string }[] = [
    { key: "all", label: "All" },
    { key: "installed", label: "Installed" },
    { key: "updates", label: "Updates" },
    { key: "not_installed", label: "Not installed" },
  ];

  return (
    <div className="shell">
      <header className={`bar ${info?.platform?.startsWith("darwin") ? "mac" : ""}`} data-tauri-drag-region>
        <div className="bar-brand">
          <Wordmark />
        </div>
        <nav className="bar-tabs">
          <span className="tab active">Launcher</span>
          <button className="tab link" onClick={() => api.openUrl(WEBSITE_URL)}>
            Yegobox apps ↗
          </button>
        </nav>
        <div className="bar-actions">
          <button className="icon-btn" onClick={checkNow} disabled={checking} title="Check for updates" aria-label="Check for updates">
            <RefreshIcon spinning={checking} />
          </button>
          <button
            className={`icon-btn ${showSettings ? "on" : ""}`}
            onClick={() => setShowSettings((s) => !s)}
            title="Settings"
            aria-label="Settings"
            aria-expanded={showSettings}
          >
            <GearIcon />
          </button>
        </div>
      </header>

      <main className="content">
        {info?.updateReady && !updateDismissed && (
          <Banner tag="Update" tone="blue" onClose={() => setUpdateDismissed(true)}>
            <span>Atlas {info.updateReady} is available.</span>
            <div className="banner-actions">
              <button className="text-link" onClick={() => api.openUrl(RELEASES_URL)}>
                What's new ↗
              </button>
              <button className="btn-solid" onClick={() => api.restart()}>
                Update and restart
              </button>
            </div>
          </Banner>
        )}

        {notice && (
          <Banner tag="Rollback" tone="amber" onClose={() => setNotice(null)}>
            <span>
              {notice.name} {notice.version} closed right after updating. Go back to {notice.previous}?
            </span>
            <div className="banner-actions">
              <button className="btn-solid" onClick={run(() => api.rollback(notice.appId).then(() => setNotice(null)))}>
                Roll back
              </button>
            </div>
          </Banner>
        )}

        {error && (
          <Banner tag="Error" tone="red" onClose={() => setError(null)}>
            <span>{error}</span>
          </Banner>
        )}

        {isAndroid && !canInstall && (
          <Banner tag="Setup" tone="amber" onClose={() => setCanInstall(true)}>
            <span>Allow Atlas to install apps, so it can install and update them for you.</span>
            <div className="banner-actions">
              <button className="btn-solid" onClick={() => api.androidOpenInstallSettings()}>
                Allow
              </button>
            </div>
          </Banner>
        )}

        {showSettings && settings && <SettingsPanel settings={settings} onChange={saveSettings} android={isAndroid} />}

        <div className="toolbar">
          <div className="segments" role="tablist">
            {filters.map((f) => (
              <button
                key={f.key}
                role="tab"
                aria-selected={filter === f.key}
                className={`segment ${filter === f.key ? "active" : ""}`}
                onClick={() => setFilter(f.key)}
              >
                {f.label} <span className="count">{counts[f.key]}</span>
              </button>
            ))}
          </div>
          <input className="search" placeholder="Search apps" value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>

        <div className="section-head">
          <span>
            01 / <b>Yegobox apps</b>
          </span>
          <span>
            {shown.length} {shown.length === 1 ? "app" : "apps"}
          </span>
        </div>

        {apps === null && <p className="empty">Loading apps…</p>}
        {apps !== null && shown.length === 0 && (
          <p className="empty">
            {apps.length === 0 ? "No apps have been published to Atlas yet. They appear here as soon as they are." : "No apps match."}
          </p>
        )}

        <div className="grid">
          {shown.map(({ app, index }) => (
            <AppCard
              key={app.id}
              app={app}
              index={index}
              progress={progress[app.id]}
              launcherUpdateReady={!!info?.updateReady}
              onInstall={run(() => api.install(app.id))}
              onOpen={run(() => api.open(app.id))}
              onRollback={run(() => api.rollback(app.id))}
              onRestart={() => api.restart()}
            />
          ))}
        </div>
      </main>

      <footer className="foot">
        <span>
          {timeAgo(info?.checkedAt ?? null, now)} · Atlas {info?.version ?? ""}
          {settings?.channel === "beta" ? " · beta channel" : ""}
        </span>
        <span className="foot-links">
          <button className="text-link" onClick={() => api.openUrl(WEBSITE_URL)}>
            Website ↗
          </button>
          <button className="text-link" onClick={() => api.openUrl(GITHUB_URL)}>
            GitHub ↗
          </button>
          <span>Installing for {platformLabel(info?.platform ?? null)}</span>
        </span>
      </footer>
    </div>
  );
}

/** ATLAS, with the launcher mark standing in for the first A (as ArtCraft does with its own). */
function Wordmark() {
  return (
    <span className="wordmark" aria-label="Atlas">
      {/* Same shapes as src/assets/logo.svg, in the text colour. */}
      <svg className="wordmark-a" viewBox="6 8 86 84" fill="currentColor" aria-hidden="true">
        <polygon points="8,92 40,8 60,8 92,92 70,92 50,36 30,92" />
        <polygon points="20,76 86,50 90,62 24,88" />
      </svg>
      <span aria-hidden="true">TLAS</span>
    </span>
  );
}

function Banner(props: { tag: string; tone: "blue" | "amber" | "red"; onClose: () => void; children: ReactNode }) {
  return (
    <div className={`banner ${props.tone}`} role={props.tone === "red" ? "alert" : "status"}>
      <span className="banner-tag">{props.tag}</span>
      <div className="banner-body">{props.children}</div>
      <button className="icon-btn small" onClick={props.onClose} aria-label="Dismiss">
        ✕
      </button>
    </div>
  );
}

function AppCard(props: {
  app: AppView;
  index: number;
  progress?: Progress;
  launcherUpdateReady: boolean;
  onInstall: () => void;
  onOpen: () => void;
  onRollback: () => void;
  onRestart: () => void;
}) {
  const { app, progress } = props;
  const [menu, setMenu] = useState(false);
  const accent = app.accent ?? ACCENTS[props.index % ACCENTS.length];
  const pct = progress?.total ? Math.round((progress.done / progress.total) * 100) : null;
  const [first, ...rest] = app.name.split(" ");

  let pill: { text: string; tone: string };
  let action: ReactNode;
  switch (app.status) {
    case "on_play":
      pill = { text: "On Google Play", tone: "blue" };
      action = (
        <button className="btn-solid wide" onClick={props.onOpen}>
          Get it on Google Play
        </button>
      );
      break;
    case "unsupported":
      pill = { text: "Not available for this computer", tone: "grey" };
      action = (
        <button className="btn-solid wide" disabled>
          Unavailable
        </button>
      );
      break;
    case "not_installed":
      pill = { text: "Available to install", tone: "blue" };
      action = (
        <button className="btn-solid wide" onClick={props.onInstall}>
          Install
        </button>
      );
      break;
    case "installing":
      pill = { text: pct === null ? "Downloading" : pct >= 100 ? "Installing" : `Downloading · ${pct}%`, tone: "blue" };
      action = (
        <button className="btn-solid wide" disabled>
          {app.installed ? "Updating…" : "Installing…"}
        </button>
      );
      break;
    case "needs_launcher_update":
      pill = { text: "Needs a newer Atlas", tone: "amber" };
      action = props.launcherUpdateReady ? (
        <button className="btn-solid wide" onClick={props.onRestart}>
          Restart Atlas
        </button>
      ) : (
        <button className="btn-solid wide" disabled>
          Updating Atlas…
        </button>
      );
      break;
    case "update_available":
      pill = { text: `Update available · ${app.available}`, tone: "amber" };
      action = (
        <button className="btn-solid wide" onClick={props.onInstall}>
          Update
        </button>
      );
      break;
    case "update_staged":
      pill = {
        text: app.running ? `${app.staged} ready · close app to finish` : `${app.staged} ready · applies on open`,
        tone: "blue",
      };
      action = (
        <button className="btn-solid wide" onClick={props.onOpen}>
          {app.running ? "Show" : "Open"}
        </button>
      );
      break;
    default:
      pill = { text: app.running ? "Installed · running" : "Installed · up to date", tone: "green" };
      action = (
        <button className="btn-solid wide" onClick={props.onOpen}>
          {app.running ? "Show" : "Open"}
        </button>
      );
  }

  const version = app.installed ?? app.available;
  const meta = [version && `Version ${version}`, formatDate(app.pubDate), !app.installed && formatBytes(app.size)]
    .filter(Boolean)
    .join(" · ");

  return (
    <article className="card" style={{ ["--accent" as string]: accent }}>
      <div className="card-top">
        <span className="badge">{String(props.index + 1).padStart(2, "0")}</span>
        {app.website && (
          <button className="text-link" onClick={() => api.openUrl(app.website!)}>
            Website ↗
          </button>
        )}
      </div>
      <div className="card-body">
        <div className="card-id">
          {app.icon ? (
            <img className="app-icon" src={app.icon} alt="" width={56} height={56} />
          ) : (
            <span className="app-icon fallback">{app.name[0]}</span>
          )}
          <div className="card-names">
            <h2>
              {first}
              {rest.length > 0 && <span className="accent"> {rest.join(" ")}</span>}
            </h2>
            {app.description && <p className="desc">{app.description}</p>}
          </div>
        </div>

        <span className={`pill ${pill.tone}`}>{pill.text}</span>
        {app.status === "installing" && (
          <div className="bar-track" aria-label="progress">
            <div style={{ width: `${pct ?? 4}%` }} />
          </div>
        )}
        {meta && <p className="meta">{meta}</p>}

        <div className="card-actions">
          {action}
          <div className="more">
            <button className="btn-outline square" onClick={() => setMenu((m) => !m)} aria-label="More" aria-expanded={menu}>
              …
            </button>
            {menu && (
              <div className="menu" onMouseLeave={() => setMenu(false)}>
                {app.status === "update_available" && (
                  <button
                    onClick={() => {
                      setMenu(false);
                      props.onOpen();
                    }}
                  >
                    Open {app.installed}
                  </button>
                )}
                {app.previous && !app.running && (
                  <button
                    onClick={() => {
                      setMenu(false);
                      props.onRollback();
                    }}
                  >
                    Roll back to {app.previous}
                  </button>
                )}
                <button
                  onClick={() => {
                    setMenu(false);
                    api.openUrl(RELEASES_URL);
                  }}
                >
                  Release notes ↗
                </button>
              </div>
            )}
          </div>
        </div>
        {app.notes && app.status !== "up_to_date" && <p className="notes">{app.notes}</p>}
      </div>
    </article>
  );
}

function SettingsPanel({ settings, onChange, android }: { settings: Settings; onChange: (s: Settings) => void; android: boolean }) {
  return (
    <section className="settings">
      <span className="banner-tag plain">Settings</span>
      <label>
        <span className="label">Channel</span>
        <select value={settings.channel} onChange={(e) => onChange({ ...settings, channel: e.target.value as Settings["channel"] })}>
          <option value="stable">Stable</option>
          <option value="beta">Beta (early builds)</option>
        </select>
      </label>
      <label className="check">
        <input type="checkbox" checked={settings.auto_update} onChange={(e) => onChange({ ...settings, auto_update: e.target.checked })} />
        <span className="label">Auto-update apps</span>
      </label>
      {/* Android checks in the background on its own schedule. */}
      {!android && (
        <label className="check">
          <input
            type="checkbox"
            checked={settings.launch_at_login}
            onChange={(e) => onChange({ ...settings, launch_at_login: e.target.checked })}
          />
          <span className="label">Start at login</span>
        </label>
      )}
    </section>
  );
}

function RefreshIcon({ spinning }: { spinning: boolean }) {
  return (
    <svg
      className={spinning ? "spin" : ""}
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M20 12a8 8 0 1 1-2.34-5.66" />
      <path d="M20 4v5h-5" />
    </svg>
  );
}

/** Feather "settings" icon (MIT). */
function GearIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}
