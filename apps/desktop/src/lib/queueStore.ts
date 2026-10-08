// External store for the download queue. Progress batches mutate rows in place
// and notify React once per batch, so 100+ live rows stay cheap to render.
import { useSyncExternalStore } from "react";
import type { DownloadTask, ProgressEvent } from "./types";

export interface QueueRow extends DownloadTask {
  speedBps: number;
  etaSecs: number | null;
  muxPercent: number | null;
  muxLabel: string | null;
}

let rows: QueueRow[] = [];
const byId = new Map<string, QueueRow>();
const listeners = new Set<() => void>();

const notify = () => {
  rows = [...byId.values()].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  listeners.forEach((l) => l());
};

const toRow = (t: DownloadTask): QueueRow => ({ ...t, speedBps: 0, etaSecs: null, muxPercent: null, muxLabel: null });

export const queueStore = {
  replaceAll(tasks: DownloadTask[]) {
    byId.clear();
    tasks.forEach((t) => byId.set(t.id, toRow(t)));
    notify();
  },
  upsert(tasks: DownloadTask[]) {
    tasks.forEach((t) => byId.set(t.id, { ...(byId.get(t.id) ?? toRow(t)), ...t }));
    notify();
  },
  apply(batch: ProgressEvent[]) {
    let changed = false;
    for (const e of batch) {
      const prev = byId.get(e.taskId);
      if (!prev) continue;
      const row = { ...prev };
      switch (e.kind) {
        case "download":
          row.bytesDone = e.bytesDone;
          row.bytesTotal = e.bytesTotal ?? row.bytesTotal;
          row.speedBps = e.speedBps;
          row.etaSecs = e.etaSecs;
          break;
        case "muxing":
          row.muxPercent = e.percent;
          row.muxLabel = e.label;
          break;
        case "state":
          row.state = e.state;
          row.error = e.message;
          if (e.state !== "downloading") row.speedBps = 0;
          break;
        case "organized":
          row.destination = e.destination;
          break;
      }
      byId.set(row.id, row);
      changed = true;
    }
    if (changed) notify();
  },
  subscribe(l: () => void) {
    listeners.add(l);
    return () => void listeners.delete(l);
  },
  snapshot: () => rows,
};

export function useQueue(): QueueRow[] {
  return useSyncExternalStore(queueStore.subscribe, queueStore.snapshot);
}
