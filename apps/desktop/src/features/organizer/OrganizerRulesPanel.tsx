import { useEffect, useState, type DragEvent } from "react";
import { FolderPlus, GripVertical, Loader2, Save, Wand2, X } from "lucide-react";
import { api, errorMessage } from "../../lib/api";
import type { OrganizerRules } from "../../lib/types";
import { Toggle } from "../../components/Toggle";
import { TOKENS, isToken, parsePattern, serializePattern, type Part, type Segment } from "./pattern";

/** Drag payloads: a palette token, or an existing chip at (segment, index). */
type DragData = { from: "palette"; token: string } | { from: "chip"; seg: number; idx: number };
const MIME = "application/x-downasaur-token";

const TOKEN_TINT: Record<string, string> = {
  Platform: "from-rose-400/25 to-rose-400/10 text-rose-100 ring-rose-400/30",
  Author: "from-amber-400/25 to-amber-400/10 text-amber-100 ring-amber-400/30",
  Year: "from-emerald-400/25 to-emerald-400/10 text-emerald-100 ring-emerald-400/30",
  Month: "from-emerald-400/25 to-emerald-400/10 text-emerald-100 ring-emerald-400/30",
  Day: "from-emerald-400/25 to-emerald-400/10 text-emerald-100 ring-emerald-400/30",
  Date: "from-teal-400/25 to-teal-400/10 text-teal-100 ring-teal-400/30",
  Resolution: "from-cyan-400/25 to-cyan-400/10 text-cyan-100 ring-cyan-400/30",
  MediaType: "from-violet-400/25 to-violet-400/10 text-violet-100 ring-violet-400/30",
};

export function OrganizerRulesPanel() {
  const [rules, setRules] = useState<OrganizerRules | null>(null);
  const [segments, setSegments] = useState<Segment[]>([]);
  const [preview, setPreview] = useState("");
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState<number | "new" | null>(null);

  useEffect(() => {
    api.getOrganizerRules().then((r) => {
      setRules(r);
      setSegments(parsePattern(r.pattern));
    });
  }, []);

  const pattern = serializePattern(segments);
  const draft = rules ? { ...rules, pattern } : null;
  const draftKey = JSON.stringify(draft);

  useEffect(() => {
    if (!draftKey || draftKey === "null") return;
    const t = setTimeout(
      () => api.previewOrganizerPath(JSON.parse(draftKey) as OrganizerRules).then(setPreview).catch(() => setPreview("")),
      120,
    );
    return () => clearTimeout(t);
  }, [draftKey]);

  if (!rules || !draft) {
    return <Loader2 className="m-auto size-5 animate-spin text-slate-500" />;
  }

  const update = (patch: Partial<OrganizerRules>) => setRules({ ...rules, ...patch });

  const readDrag = (e: DragEvent): DragData | null => {
    try {
      return JSON.parse(e.dataTransfer.getData(MIME)) as DragData;
    } catch {
      return null;
    }
  };

  const drop = (e: DragEvent, target: number | "new") => {
    e.preventDefault();
    setDragOver(null);
    const data = readDrag(e);
    if (!data) return;
    const next = segments.map((s) => [...s]);
    let part: Part;
    if (data.from === "palette") {
      if (!isToken(data.token)) return;
      part = { kind: "token", name: data.token };
    } else {
      const [moved] = next[data.seg]?.splice(data.idx, 1) ?? [];
      if (!moved) return;
      part = moved;
    }
    if (target === "new") next.push([part]);
    else {
      const seg = next[target];
      if (!seg) return;
      // Separate adjacent tokens so `{Year}{Month}` doesn't become `202703`.
      if (seg.length && seg[seg.length - 1]?.kind === "token" && part.kind === "token") seg.push({ kind: "literal", value: "-" });
      seg.push(part);
    }
    setSegments(next.filter((s) => s.length));
  };

  const removePart = (seg: number, idx: number) =>
    setSegments(segments.map((s, i) => (i === seg ? s.filter((_, j) => j !== idx) : s)).filter((s) => s.length));

  const save = async () => {
    setSaving(true);
    setMessage(null);
    try {
      await api.setOrganizerRules(draft);
      setMessage("Saved. New downloads use this layout.");
    } catch (e) {
      setMessage(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col gap-4 overflow-y-auto pb-6">
      <header className="flex items-end justify-between">
        <div>
          <h1 className="text-xl font-semibold text-white">Auto-Organizer rules</h1>
          <p className="text-sm text-slate-400">Drag tokens into folders to design where finished downloads land.</p>
        </div>
        <button className="btn-primary" onClick={save} disabled={saving}>
          {saving ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />} Save rules
        </button>
      </header>

      <section className="panel p-5">
        <div className="mb-3 text-xs font-semibold uppercase tracking-wider text-slate-500">Tokens</div>
        <div className="flex flex-wrap gap-2">
          {TOKENS.map((t) => (
            <span
              key={t}
              draggable
              onDragStart={(e) => e.dataTransfer.setData(MIME, JSON.stringify({ from: "palette", token: t } satisfies DragData))}
              className={`cursor-grab select-none rounded-lg bg-gradient-to-b px-3 py-1.5 font-mono text-sm ring-1 active:cursor-grabbing ${TOKEN_TINT[t]}`}
            >
              [{t}]
            </span>
          ))}
        </div>

        <div className="mb-3 mt-6 text-xs font-semibold uppercase tracking-wider text-slate-500">Folder structure</div>
        <div className="flex flex-wrap items-center gap-2 rounded-xl border border-white/[0.06] bg-ink-850/60 p-3">
          <span className="rounded-lg bg-ink-700 px-3 py-2 font-mono text-sm text-slate-300">{"{TargetDir}"}</span>
          {segments.map((seg, si) => (
            <div key={si} className="flex items-center gap-2">
              <span className="text-slate-600">/</span>
              <div
                onDragOver={(e) => {
                  e.preventDefault();
                  setDragOver(si);
                }}
                onDragLeave={() => setDragOver(null)}
                onDrop={(e) => drop(e, si)}
                className={`flex min-h-10 items-center gap-1 rounded-lg border border-dashed px-2 py-1 transition ${
                  dragOver === si ? "border-emerald-400 bg-emerald-400/10" : "border-white/15"
                }`}
              >
                {seg.map((p, pi) =>
                  p.kind === "token" ? (
                    <span
                      key={pi}
                      draggable
                      onDragStart={(e) => e.dataTransfer.setData(MIME, JSON.stringify({ from: "chip", seg: si, idx: pi } satisfies DragData))}
                      className={`group inline-flex cursor-grab items-center gap-1 rounded-md bg-gradient-to-b px-2 py-1 font-mono text-xs ring-1 ${TOKEN_TINT[p.name]}`}
                    >
                      <GripVertical className="size-3 opacity-50" />[{p.name}]
                      <button onClick={() => removePart(si, pi)} className="opacity-0 transition group-hover:opacity-100" aria-label={`Remove ${p.name}`}>
                        <X className="size-3" />
                      </button>
                    </span>
                  ) : (
                    <input
                      key={pi}
                      value={p.value}
                      onChange={(e) =>
                        setSegments(segments.map((s, i) => (i === si ? s.map((x, j) => (j === pi ? { kind: "literal", value: e.target.value } : x)) : s)))
                      }
                      className="w-[3ch] min-w-[2ch] rounded bg-transparent text-center font-mono text-xs text-slate-300 focus:bg-ink-700 focus:outline-none"
                      style={{ width: `${Math.max(2, p.value.length + 1)}ch` }}
                    />
                  ),
                )}
              </div>
            </div>
          ))}
          <div
            onDragOver={(e) => {
              e.preventDefault();
              setDragOver("new");
            }}
            onDragLeave={() => setDragOver(null)}
            onDrop={(e) => drop(e, "new")}
            className={`flex min-h-10 items-center gap-1.5 rounded-lg border border-dashed px-3 text-xs transition ${
              dragOver === "new" ? "border-emerald-400 bg-emerald-400/10 text-emerald-200" : "border-white/10 text-slate-500"
            }`}
          >
            <FolderPlus className="size-3.5" /> Drop for new folder
          </div>
        </div>

        <div className="mt-4 rounded-xl bg-black/30 p-3">
          <div className="mb-1 flex items-center gap-1.5 text-[11px] uppercase tracking-wider text-slate-500">
            <Wand2 className="size-3" /> Live preview (8K YouTube sample)
          </div>
          <code className="break-all font-mono text-sm text-emerald-200">{preview || "…"}</code>
        </div>
      </section>

      <div className="grid grid-cols-2 gap-4">
        <section className="panel space-y-3 p-5">
          <h2 className="font-medium text-white">Destination</h2>
          <input className="input font-mono" value={rules.targetDir} onChange={(e) => update({ targetDir: e.target.value })} />
          <Toggle label="Organize finished downloads" checked={rules.enabled} onChange={(enabled) => update({ enabled })} />
          <Toggle label="Clean file names (strip emojis, hashtags, clickbait)" checked={rules.cleanNames} onChange={(cleanNames) => update({ cleanNames })} />
        </section>

        <section className="panel space-y-3 p-5">
          <Toggle
            label="Separate media types"
            checked={rules.separation.enabled}
            onChange={(enabled) => update({ separation: { ...rules.separation, enabled } })}
          />
          {(
            [
              ["audioDir", "Audio only"],
              ["eightKDir", "8K video"],
              ["shortsDir", "Shorts, Reels & Stories"],
            ] as const
          ).map(([key, label]) => (
            <label key={key} className="grid grid-cols-[140px_1fr] items-center gap-3 text-sm text-slate-400">
              {label}
              <input
                className="input py-1.5 font-mono"
                disabled={!rules.separation.enabled}
                value={rules.separation[key]}
                onChange={(e) => update({ separation: { ...rules.separation, [key]: e.target.value } })}
              />
            </label>
          ))}
        </section>
      </div>

      {message && <div className="text-sm text-slate-400">{message}</div>}
    </div>
  );
}
