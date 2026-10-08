import { useMemo, useState } from "react";
import { Check, Droplets, Film, Loader2, Music, X } from "lucide-react";
import { api, errorMessage } from "../../lib/api";
import { queueStore } from "../../lib/queueStore";
import type { Container, DownloadProfile, MediaInfo, Resolution } from "../../lib/types";
import { PlatformBadge } from "../../components/PlatformBadge";

interface ProfileOption {
  key: string;
  title: string;
  subtitle: string;
  icon: typeof Film;
  profile: (container: Container) => DownloadProfile;
  hero?: boolean;
  available: boolean;
}

const video = (maxResolution: Resolution) => (container: Container): DownloadProfile => ({ type: "video", maxResolution, container });

export function FormatPickerModal({ items, onClose, onQueued }: { items: MediaInfo[]; onClose: () => void; onQueued: () => void }) {
  const [selected, setSelected] = useState(() => new Set(items.map((i) => i.sourceUrl)));
  const [container, setContainer] = useState<Container>("mkv");
  const [choice, setChoice] = useState<string>("8k");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const maxHeight = Math.max(0, ...items.flatMap((i) => i.formats.map((f) => Math.min(f.height ?? 0, f.width ?? Infinity))));
  const watermarkPlatform = items.some((i) => i.platform === "tiktok" || i.platform === "instagram");

  const options = useMemo<ProfileOption[]>(() => {
    const list: ProfileOption[] = [
      { key: "8k", title: "Ultra HD 8K (4320p)", subtitle: "AV1/VP9 + best audio, lossless remux", icon: Film, profile: video("4320p"), hero: true, available: maxHeight >= 4320 || items.length > 1 },
      { key: "4k", title: "4K (2160p)", subtitle: "HDR when available", icon: Film, profile: video("2160p"), available: true },
      { key: "1080", title: "Full HD (1080p)", subtitle: "Best compatibility", icon: Film, profile: video("1080p"), available: true },
      { key: "mp3", title: "Download Audio Only (MP3)", subtitle: "Highest-bitrate audio, VBR V0", icon: Music, profile: () => ({ type: "audioOnly", format: "mp3" }), available: true },
    ];
    if (watermarkPlatform) {
      list.splice(1, 0, {
        key: "nowm",
        title: "Download TikTok without Watermark",
        subtitle: "Clean source rendition, no overlay",
        icon: Droplets,
        profile: () => ({ type: "noWatermark" }),
        hero: true,
        available: true,
      });
    }
    return list;
  }, [maxHeight, items.length, watermarkPlatform]);

  const current = options.find((o) => o.key === choice && o.available) ?? options.find((o) => o.available);
  const isVideo = current?.profile(container).type === "video";

  const submit = async () => {
    if (!current) return;
    setBusy(true);
    setError(null);
    try {
      const chosen = items.filter((i) => selected.has(i.sourceUrl));
      queueStore.upsert(await api.enqueueDownload(chosen, current.profile(container)));
      onQueued();
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 p-6 backdrop-blur-sm" onClick={onClose}>
      <div className="panel flex max-h-[85vh] w-full max-w-2xl flex-col overflow-hidden" onClick={(e) => e.stopPropagation()} role="dialog" aria-modal>
        <header className="flex items-center justify-between border-b border-white/[0.06] px-5 py-4">
          <div>
            <h2 className="text-lg font-semibold text-white">Choose a download profile</h2>
            <p className="text-sm text-slate-400">
              {items.length === 1 ? items[0]?.title : `${selected.size} of ${items.length} items selected`}
            </p>
          </div>
          <button className="btn-ghost p-2" onClick={onClose} aria-label="Close">
            <X className="size-4" />
          </button>
        </header>

        <div className="grid gap-2 overflow-y-auto p-5">
          {options.map((o) => {
            const active = current?.key === o.key;
            return (
              <button
                key={o.key}
                disabled={!o.available}
                onClick={() => setChoice(o.key)}
                className={`flex items-center gap-4 rounded-xl border p-4 text-left transition disabled:opacity-40 ${
                  active
                    ? "border-emerald-400/60 bg-emerald-400/[0.08]"
                    : o.hero
                      ? "border-cyan-400/20 bg-gradient-to-r from-cyan-400/[0.06] to-transparent hover:border-cyan-400/40"
                      : "border-white/[0.06] bg-ink-850/60 hover:border-white/15"
                }`}
              >
                <div className={`grid size-10 place-items-center rounded-lg ${o.hero ? "bg-gradient-to-br from-emerald-400 to-cyan-400 text-ink-950" : "bg-ink-700 text-slate-300"}`}>
                  <o.icon className="size-5" />
                </div>
                <div className="flex-1">
                  <div className="font-medium text-white">{o.title}</div>
                  <div className="text-sm text-slate-400">{o.available ? o.subtitle : `Not offered by this source (max ${maxHeight}p)`}</div>
                </div>
                {active && <Check className="size-5 text-emerald-300" />}
              </button>
            );
          })}

          {isVideo && (
            <div className="mt-2 flex items-center gap-3 text-sm text-slate-400">
              Container
              {(["mkv", "mp4"] as const).map((c) => (
                <button key={c} onClick={() => setContainer(c)} className={`chip px-3 py-1 uppercase ${container === c ? "bg-white/10 text-white" : "hover:bg-white/5"}`}>
                  {c}
                </button>
              ))}
              <span className="text-xs text-slate-500">MKV keeps soft subtitles and every codec as-is.</span>
            </div>
          )}

          {items.length > 1 && (
            <div className="mt-3 max-h-56 overflow-y-auto rounded-xl border border-white/[0.06]">
              {items.map((i) => (
                <label key={i.sourceUrl} className="flex cursor-pointer items-center gap-3 border-b border-white/[0.04] px-3 py-2 text-sm last:border-0 hover:bg-white/[0.03]">
                  <input
                    type="checkbox"
                    className="accent-emerald-400"
                    checked={selected.has(i.sourceUrl)}
                    onChange={(e) => {
                      const next = new Set(selected);
                      if (e.target.checked) next.add(i.sourceUrl);
                      else next.delete(i.sourceUrl);
                      setSelected(next);
                    }}
                  />
                  <PlatformBadge platform={i.platform} size="sm" />
                  <span className="truncate">{i.title}</span>
                </label>
              ))}
            </div>
          )}
          {error && <div className="rounded-lg bg-rose-500/10 p-3 text-sm text-rose-200">{error}</div>}
        </div>

        <footer className="flex justify-end gap-2 border-t border-white/[0.06] px-5 py-4">
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={submit} disabled={busy || selected.size === 0 || !current}>
            {busy && <Loader2 className="size-4 animate-spin" />}
            Add {selected.size > 1 ? `${selected.size} downloads` : "to queue"}
          </button>
        </footer>
      </div>
    </div>
  );
}
