import { useEffect, useRef, useState } from "react";
import { ClipboardPaste, Clock, Layers, Loader2, ShieldAlert, Sparkles, User } from "lucide-react";
import { api, errorMessage, isDrmError, readClipboard } from "../../lib/api";
import { detectPlatform, PLATFORMS } from "../../lib/platforms";
import { formatDuration } from "../../lib/format";
import type { Extraction, MediaInfo } from "../../lib/types";
import { PlatformBadge } from "../../components/PlatformBadge";
import { FormatPickerModal } from "../formats/FormatPickerModal";

type Status =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; extraction: Extraction }
  | { kind: "error"; message: string; drm: boolean };

export function LinkSniffer() {
  const [input, setInput] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [picker, setPicker] = useState<MediaInfo[] | null>(null);
  const [expanding, setExpanding] = useState(false);
  const requestId = useRef(0);
  const platform = detectPlatform(input);

  // Fetch metadata shortly after the input settles on a valid link.
  useEffect(() => {
    if (!platform) {
      setStatus({ kind: "idle" });
      return;
    }
    const id = ++requestId.current;
    const timer = setTimeout(async () => {
      setStatus({ kind: "loading" });
      try {
        const extraction = await api.fetchMetadata(input);
        if (id === requestId.current) setStatus({ kind: "ready", extraction });
      } catch (e) {
        if (id === requestId.current) setStatus({ kind: "error", message: errorMessage(e), drm: isDrmError(e) });
      }
    }, 350);
    return () => clearTimeout(timer);
  }, [input, platform]);

  // Auto-sniff the clipboard when the window regains focus.
  useEffect(() => {
    const onFocus = async () => {
      try {
        const text = (await readClipboard()).trim();
        if (text && text !== input && detectPlatform(text)) setInput(text);
      } catch {
        /* clipboard permission denied: ignore */
      }
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [input]);

  const paste = async () => setInput((await readClipboard().catch(() => "")).trim());

  const openPicker = async () => {
    if (status.kind !== "ready") return;
    const ex = status.extraction;
    if (ex.type === "media") {
      setPicker([ex]);
      return;
    }
    setExpanding(true);
    try {
      setPicker(await api.expandCollection(ex.entries));
    } finally {
      setExpanding(false);
    }
  };

  return (
    <section className="panel relative overflow-hidden p-5">
      <div className="pointer-events-none absolute -right-20 -top-24 size-72 rounded-full bg-cyan-400/10 blur-3xl" />
      <div className="mb-3 flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.18em] text-slate-400">
        <Sparkles className="size-3.5 text-emerald-300" /> Smart link sniffer
      </div>

      <div className="flex gap-3">
        <div className="relative flex-1">
          <div className="absolute left-2 top-1/2 -translate-y-1/2">
            {platform ? <PlatformBadge platform={platform} /> : <span className="grid size-8 place-items-center text-slate-500">⌁</span>}
          </div>
          <input
            className="input h-12 pl-12 text-[15px]"
            placeholder="Paste a YouTube, TikTok, Instagram, Twitch, X, Facebook or any video link…"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            spellCheck={false}
            autoFocus
          />
        </div>
        <button className="btn-ghost h-12 border border-white/10" onClick={paste}>
          <ClipboardPaste className="size-4" /> Paste
        </button>
      </div>

      <div className="mt-4 min-h-[112px]">
        {status.kind === "idle" && (
          <p className="pt-8 text-center text-sm text-slate-500">
            Copy a link anywhere. Downasaur detects the platform the moment this window gets focus.
          </p>
        )}
        {status.kind === "loading" && (
          <div className="flex items-center gap-3 pt-8 text-sm text-slate-400">
            <Loader2 className="size-4 animate-spin text-cyan-300" /> Fetching metadata from{" "}
            {platform ? PLATFORMS[platform].label : "the web"}…
          </div>
        )}
        {status.kind === "error" && (
          <div
            className={`mt-2 flex items-start gap-3 rounded-xl border p-4 text-sm ${
              status.drm ? "border-amber-400/30 bg-amber-400/10 text-amber-100" : "border-rose-400/30 bg-rose-400/10 text-rose-100"
            }`}
          >
            <ShieldAlert className="mt-0.5 size-4 shrink-0" />
            <div>
              <div className="font-semibold">{status.drm ? "DRM Protected Content" : "Couldn't read this link"}</div>
              <div className="opacity-80">{status.drm ? status.message.replace(/^DRM Protected Content:\s*/, "") : status.message}</div>
            </div>
          </div>
        )}
        {status.kind === "ready" && (
          <MetadataCard extraction={status.extraction} busy={expanding} onDownload={openPicker} />
        )}
      </div>

      {picker && <FormatPickerModal items={picker} onClose={() => setPicker(null)} onQueued={() => setInput("")} />}
    </section>
  );
}

function MetadataCard({ extraction, busy, onDownload }: { extraction: Extraction; busy: boolean; onDownload: () => void }) {
  if (extraction.type === "collection") {
    return (
      <div className="flex items-center gap-4 rounded-xl border border-white/[0.06] bg-ink-850/70 p-4">
        <div className="grid size-16 place-items-center rounded-xl bg-ink-800">
          <Layers className="size-7 text-cyan-300" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate font-semibold text-white">{extraction.title}</div>
          <div className="text-sm text-slate-400">{extraction.entries.length} videos{extraction.continuation ? "+ (more pages)" : ""}</div>
        </div>
        <button className="btn-primary" onClick={onDownload} disabled={busy}>
          {busy && <Loader2 className="size-4 animate-spin" />} Choose formats
        </button>
      </div>
    );
  }

  const maxH = Math.max(0, ...extraction.formats.map((f) => f.height ?? 0));
  return (
    <div className="flex items-center gap-4 rounded-xl border border-white/[0.06] bg-ink-850/70 p-3">
      <div className="relative aspect-video w-48 shrink-0 overflow-hidden rounded-lg bg-gradient-to-br from-ink-700 to-ink-800">
        {extraction.thumbnail ? (
          <img src={extraction.thumbnail} alt="" className="size-full object-cover" />
        ) : (
          <div className="grid size-full place-items-center">
            <PlatformBadge platform={extraction.platform} size="lg" />
          </div>
        )}
        {maxH >= 4320 && (
          <span className="chip absolute left-1.5 top-1.5 bg-black/70 font-bold text-emerald-300 backdrop-blur">8K</span>
        )}
        <span className="chip absolute bottom-1.5 right-1.5 bg-black/70 text-white backdrop-blur">
          {formatDuration(extraction.durationSecs)}
        </span>
      </div>
      <div className="min-w-0 flex-1">
        <div className="line-clamp-2 font-semibold leading-snug text-white">{extraction.title}</div>
        <div className="mt-1.5 flex flex-wrap items-center gap-3 text-sm text-slate-400">
          <span className="inline-flex items-center gap-1">
            <User className="size-3.5" /> {extraction.author ?? "Unknown"}
          </span>
          <span className="inline-flex items-center gap-1">
            <Clock className="size-3.5" /> {formatDuration(extraction.durationSecs)}
          </span>
          {maxH > 0 && <span className="chip bg-white/5 text-slate-300">up to {maxH}p</span>}
        </div>
      </div>
      <button className="btn-primary" onClick={onDownload}>
        Download
      </button>
    </div>
  );
}
