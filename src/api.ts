// Typed wrappers over the Rust commands in src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

export type AppStatus =
  | "unsupported"
  | "not_installed"
  | "installing"
  | "needs_launcher_update"
  | "update_available"
  | "update_staged"
  | "up_to_date"
  /** Android: the app ships through Google Play and isn't installed. */
  | "on_play";

export interface AppView {
  id: string;
  name: string;
  icon: string | null;
  description: string | null;
  website: string | null;
  accent: string | null;
  pubDate: string | null;
  installed: string | null;
  staged: string | null;
  previous: string | null;
  available: string | null;
  notes: string;
  size: number;
  status: AppStatus;
  running: boolean;
}

export interface Settings {
  channel: "stable" | "beta";
  auto_update: boolean;
  launch_at_login: boolean;
}

export interface Progress {
  id: string;
  done: number;
  total: number | null;
}

export interface Notice {
  appId: string;
  name: string;
  version: string;
  previous: string;
}

export interface LauncherInfo {
  version: string;
  platform: string | null;
  updateReady: string | null;
  checkedAt: number | null;
}

export const api = {
  listApps: () => invoke<AppView[]>("list_apps"),
  refreshApps: () => invoke<AppView[]>("refresh_apps"),
  install: (id: string) => invoke<void>("install_app", { id }),
  open: (id: string) => invoke<boolean>("open_app", { id }),
  rollback: (id: string) => invoke<string>("rollback_app", { id }),
  takeNotice: () => invoke<Notice | null>("take_notice"),
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  launcherInfo: () => invoke<LauncherInfo>("launcher_info"),
  checkForUpdates: () => invoke<void>("check_for_updates"),
  restart: () => invoke<void>("restart_launcher"),
  /** Android only: may Atlas install apps ("Install unknown apps")? */
  androidCanInstall: () => invoke<boolean>("android_can_install"),
  androidOpenInstallSettings: () => invoke<void>("android_open_install_settings"),
  openUrl: (url: string) => openUrl(url),
};

export const RELEASES_URL = "https://github.com/yegobox/flipper-launcher/releases";
export const WEBSITE_URL = "https://yegobox.com";
export const GITHUB_URL = "https://github.com/yegobox/flipper-launcher";

export function platformLabel(key: string | null): string {
  if (!key) return "this computer";
  if (key.startsWith("darwin")) return "macOS";
  if (key.startsWith("windows")) return "Windows";
  if (key.startsWith("linux")) return "Linux";
  if (key === "android") return "Android";
  return key;
}

export function formatDate(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  return isNaN(d.getTime()) ? "" : d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

export function timeAgo(ms: number | null, now: number): string {
  if (!ms) return "not checked yet";
  const min = Math.floor((now - ms) / 60000);
  if (min < 1) return "checked just now";
  if (min < 60) return `checked ${min} min ago`;
  const h = Math.floor(min / 60);
  return `checked ${h} h ago`;
}

export function formatBytes(n: number): string {
  if (!n) return "";
  const mb = n / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb.toFixed(0)} MB`;
}
