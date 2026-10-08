// Typed bridge to the Rust commands in apps/desktop/src-tauri/src/commands.
// Outside Tauri (plain `vite dev` in a browser) it falls back to a mock backend
// so the layout can be developed and previewed without the native shell.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { mock } from "./mock";
import type {
  DownloadProfile,
  DownloadTask,
  Extraction,
  MediaInfo,
  OrganizerRules,
  ProgressEvent,
  SchedulerConfig,
  SniffResult,
} from "./types";

export const PROGRESS_EVENT = "downasaur://progress";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const api = {
  sniffLink: (input: string) => (isTauri ? invoke<SniffResult>("sniff_link", { input }) : mock.sniffLink(input)),
  fetchMetadata: (input: string) =>
    isTauri ? invoke<Extraction>("fetch_metadata", { input }) : mock.fetchMetadata(input),
  expandCollection: (entries: string[]) =>
    isTauri ? invoke<MediaInfo[]>("expand_collection", { entries }) : mock.expandCollection(entries),
  enqueueDownload: (items: MediaInfo[], profile: DownloadProfile) =>
    isTauri ? invoke<DownloadTask[]>("enqueue_download", { items, profile }) : mock.enqueue(items, profile),
  listDownloads: () => (isTauri ? invoke<DownloadTask[]>("list_downloads") : mock.list()),
  pauseDownload: (id: string) => (isTauri ? invoke<void>("pause_download", { id }) : mock.pause(id)),
  resumeDownload: (id: string) => (isTauri ? invoke<void>("resume_download", { id }) : mock.resume(id)),
  cancelDownload: (id: string) => (isTauri ? invoke<void>("cancel_download", { id }) : mock.cancel(id)),
  getOrganizerRules: () => (isTauri ? invoke<OrganizerRules>("get_organizer_rules") : mock.getRules()),
  setOrganizerRules: (rules: OrganizerRules) =>
    isTauri ? invoke<void>("set_organizer_rules", { rules }) : mock.setRules(rules),
  previewOrganizerPath: (rules: OrganizerRules) =>
    isTauri ? invoke<string>("preview_organizer_path", { rules }) : mock.previewPath(rules),
  getSchedulerConfig: () => (isTauri ? invoke<SchedulerConfig>("get_scheduler_config") : mock.getScheduler()),
  setSchedulerConfig: (config: SchedulerConfig) =>
    isTauri ? invoke<void>("set_scheduler_config", { config }) : mock.setScheduler(config),
};

export function onProgress(handler: (batch: ProgressEvent[]) => void): Promise<UnlistenFn> {
  if (isTauri) return listen<ProgressEvent[]>(PROGRESS_EVENT, (e) => handler(e.payload));
  return Promise.resolve(mock.subscribe(handler));
}

export async function readClipboard(): Promise<string> {
  if (isTauri) {
    const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
    return readText();
  }
  return navigator.clipboard?.readText?.() ?? "";
}

export async function revealInFolder(path: string): Promise<void> {
  if (!isTauri) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

export function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

export function isDrmError(e: unknown): boolean {
  return !!e && typeof e === "object" && (e as { code?: string }).code === "drm_protected";
}
