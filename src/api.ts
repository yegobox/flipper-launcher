// Typed wrappers over the Rust commands in src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";

export type AppStatus =
  | "unsupported"
  | "not_installed"
  | "installing"
  | "needs_launcher_update"
  | "update_available"
  | "update_staged"
  | "up_to_date";

export interface AppView {
  id: string;
  name: string;
  icon: string | null;
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
};

export function formatBytes(n: number): string {
  if (!n) return "";
  const mb = n / (1024 * 1024);
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb.toFixed(0)} MB`;
}
