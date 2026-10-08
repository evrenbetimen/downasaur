import { memo, useRef, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { FolderOpen, Pause, Play, X } from "lucide-react";
import { api, revealInFolder } from "../../lib/api";
import { formatBytes, formatDuration, formatSpeed, percent } from "../../lib/format";
import { useQueue, type QueueRow } from "../../lib/queueStore";
import type { DownloadProfile, TaskState } from "../../lib/types";
import { PlatformBadge } from "../../components/PlatformBadge";

const ROW_HEIGHT = 64;
const COLUMNS = "grid-cols-[minmax(260px,2.4fr)_120px_minmax(180px,1.4fr)_96px_72px_minmax(160px,1.4fr)_96px]";

const STATE_STYLE: Record<TaskState, { label: string; cls: string }> = {
  queued: { label: "Queued", cls: "bg-slate-500/15 text-slate-300" },
  scheduled: { label: "Scheduled", cls: "bg-indigo-500/15 text-indigo-300" },
  fetching: { label: "Fetching", cls: "bg-sky-500/15 text-sky-300" },
  downloading: { label: "Downloading", cls: "bg-cyan-500/15 text-cyan-300" },
  paused: { label: "Paused", cls: "bg-amber-500/15 text-amber-300" },
  muxing: { label: "Muxing", cls: "bg-fuchsia-500/15 text-fuchsia-300" },
  organizing: { label: "Organizing", cls: "bg-violet-500/15 text-violet-300" },
  completed: { label: "Completed", cls: "bg-emerald-500/15 text-emerald-300" },
  failed: { label: "Failed", cls: "bg-rose-500/15 text-rose-300" },
  cancelled: { label: "Cancelled", cls: "bg-slate-500/15 text-slate-400" },
};

function profileLabel(p: DownloadProfile): string {
  switch (p.type) {
    case "video":
      return `${p.maxResolution === "4320p" ? "8K" : p.maxResolution} · ${p.container.toUpperCase()}`;
    case "audioOnly":
      return `Audio · ${p.format.toUpperCase()}`;
    case "noWatermark":
      return "No watermark";
  }
}

export function QueueTable() {
  const rows = useQueue();
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    getItemKey: (i) => rows[i]?.id ?? i,
  });

  return (
    <section className="panel flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className={`grid ${COLUMNS} gap-3 border-b border-white/[0.06] px-4 py-2.5 text-[11px] font-semibold uppercase tracking-wider text-slate-500`}>
        <span>Media</span>
        <span>Status</span>
        <span>Progress</span>
        <span className="text-right">Speed</span>
        <span className="text-right">ETA</span>
        <span>Destination</span>
        <span className="text-right">Actions</span>
      </div>

      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        {rows.length === 0 ? (
          <div className="grid h-full place-items-center text-sm text-slate-500">Your queue is empty. Paste a link above to start.</div>
        ) : (
          <div style={{ height: virtualizer.getTotalSize() }} className="relative">
            {virtualizer.getVirtualItems().map((v) => {
              const row = rows[v.index];
              return row ? (
                <div key={v.key} className="absolute inset-x-0" style={{ top: v.start, height: ROW_HEIGHT }}>
                  <Row row={row} />
                </div>
              ) : null;
            })}
          </div>
        )}
      </div>
    </section>
  );
}

const Row = memo(function Row({ row }: { row: QueueRow }) {
  const pct = row.state === "muxing" && row.muxPercent != null ? row.muxPercent : percent(row.bytesDone, row.bytesTotal);
  const style = STATE_STYLE[row.state];
  const running = ["queued", "scheduled", "fetching", "downloading"].includes(row.state);
  const statusText = row.state === "muxing" ? (row.muxLabel ?? "Muxing…") : style.label;

  return (
    <div className={`grid h-full ${COLUMNS} items-center gap-3 border-b border-white/[0.04] px-4 text-sm hover:bg-white/[0.02]`}>
      <div className="flex min-w-0 items-center gap-3">
        <PlatformBadge platform={row.platform} size="sm" />
        <div className="min-w-0">
          <div className="truncate font-medium text-slate-100" title={row.title}>
            {row.title}
          </div>
          <div className="truncate text-xs text-slate-500">
            {row.author ?? "Unknown"} · {profileLabel(row.profile)}
          </div>
        </div>
      </div>

      <span className={`chip w-fit ${style.cls}`} title={row.error ?? undefined}>
        {statusText}
      </span>

      <div>
        <div className="h-1.5 overflow-hidden rounded-full bg-ink-700">
          <div
            className={`h-full rounded-full transition-[width] duration-150 ${
              row.state === "muxing" ? "bg-fuchsia-400" : row.state === "failed" ? "bg-rose-400" : "bg-gradient-to-r from-emerald-400 to-cyan-400"
            }`}
            style={{ width: `${pct}%` }}
          />
        </div>
        <div className="mt-1 font-mono text-[11px] tabular-nums text-slate-500">
          {formatBytes(row.bytesDone)} / {formatBytes(row.bytesTotal)} · {pct.toFixed(1)}%
        </div>
      </div>

      <span className="text-right font-mono text-xs tabular-nums text-slate-300">{row.state === "downloading" ? formatSpeed(row.speedBps) : "–"}</span>
      <span className="text-right font-mono text-xs tabular-nums text-slate-400">{row.state === "downloading" ? formatDuration(row.etaSecs) : "–"}</span>

      <span className="truncate font-mono text-xs text-slate-500" title={row.destination ?? undefined}>
        {row.error ? <span className="text-rose-300">{row.error}</span> : (row.destination ?? "Auto-organizer pending")}
      </span>

      <div className="flex justify-end gap-1">
        {running && (
          <IconButton label="Pause" onClick={() => api.pauseDownload(row.id)}>
            <Pause className="size-3.5" />
          </IconButton>
        )}
        {(row.state === "paused" || row.state === "failed") && (
          <IconButton label="Resume" onClick={() => api.resumeDownload(row.id)}>
            <Play className="size-3.5" />
          </IconButton>
        )}
        {row.destination && row.state === "completed" && (
          <IconButton label="Show in folder" onClick={() => revealInFolder(row.destination ?? "")}>
            <FolderOpen className="size-3.5" />
          </IconButton>
        )}
        {!["completed", "cancelled"].includes(row.state) && (
          <IconButton label="Cancel" onClick={() => api.cancelDownload(row.id)}>
            <X className="size-3.5" />
          </IconButton>
        )}
      </div>
    </div>
  );
});

function IconButton({ label, onClick, children }: { label: string; onClick: () => void; children: ReactNode }) {
  return (
    <button className="btn-ghost size-7 p-0" title={label} aria-label={label} onClick={onClick}>
      {children}
    </button>
  );
}
