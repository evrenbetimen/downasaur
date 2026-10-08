import { useEffect, useState } from "react";
import { Loader2, Plus, Save, Trash2, Wifi } from "lucide-react";
import { api, errorMessage } from "../../lib/api";
import type { SchedulerConfig, ScheduleWindow } from "../../lib/types";
import { Toggle } from "../../components/Toggle";

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const hhmm = (t: string) => t.slice(0, 5);
const toNaive = (t: string) => (t.length === 5 ? `${t}:00` : t);

export function SchedulerPanel() {
  const [config, setConfig] = useState<SchedulerConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    api.getSchedulerConfig().then(setConfig).catch((e) => setMessage(errorMessage(e)));
  }, []);

  if (!config) return <Loader2 className="m-auto size-5 animate-spin text-slate-500" />;

  const setWindow = (i: number, patch: Partial<ScheduleWindow>) =>
    setConfig({ ...config, windows: config.windows.map((w, j) => (j === i ? { ...w, ...patch } : w)) });

  const limitMb = config.bandwidthLimit ? config.bandwidthLimit / 1024 ** 2 : "";

  const save = async () => {
    setSaving(true);
    try {
      await api.setSchedulerConfig(config);
      setMessage("Saved.");
    } catch (e) {
      setMessage(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mx-auto flex h-full max-w-3xl flex-col gap-4 overflow-y-auto pb-6">
      <header className="flex items-end justify-between">
        <div>
          <h1 className="text-xl font-semibold text-white">Bandwidth scheduler</h1>
          <p className="text-sm text-slate-400">Run heavy 8K downloads overnight and protect metered connections.</p>
        </div>
        <button className="btn-primary" onClick={save} disabled={saving}>
          {saving ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />} Save
        </button>
      </header>

      <section className="panel space-y-4 p-5">
        <Toggle label="Only download inside these windows" checked={config.enabled} onChange={(enabled) => setConfig({ ...config, enabled })} />
        {config.windows.map((w, i) => (
          <div key={i} className="flex flex-wrap items-center gap-3 rounded-xl border border-white/[0.06] bg-ink-850/60 p-3 text-sm">
            <span className="text-slate-400">Start at</span>
            <input type="time" className="input w-32 py-1.5" value={hhmm(w.start)} onChange={(e) => setWindow(i, { start: toNaive(e.target.value) })} />
            <span className="text-slate-400">pause at</span>
            <input type="time" className="input w-32 py-1.5" value={hhmm(w.end)} onChange={(e) => setWindow(i, { end: toNaive(e.target.value) })} />
            <div className="flex gap-1">
              {DAYS.map((d) => {
                const on = w.days.includes(d);
                return (
                  <button
                    key={d}
                    onClick={() => setWindow(i, { days: on ? w.days.filter((x) => x !== d) : [...w.days, d] })}
                    className={`chip ${on ? "bg-emerald-400/20 text-emerald-200" : "bg-white/5 text-slate-500 hover:text-slate-300"}`}
                  >
                    {d}
                  </button>
                );
              })}
            </div>
            <button className="btn-ghost ml-auto p-2" aria-label="Remove window" onClick={() => setConfig({ ...config, windows: config.windows.filter((_, j) => j !== i) })}>
              <Trash2 className="size-4" />
            </button>
          </div>
        ))}
        <button
          className="btn-ghost border border-dashed border-white/10"
          onClick={() => setConfig({ ...config, windows: [...config.windows, { start: "02:00:00", end: "06:00:00", days: [] }] })}
        >
          <Plus className="size-4" /> Add window
        </button>
        <p className="text-xs text-slate-500">No days selected means every day. Windows can cross midnight.</p>
      </section>

      <section className="panel space-y-3 p-5">
        <label className="grid grid-cols-[1fr_160px] items-center gap-3 text-sm text-slate-300">
          Global bandwidth cap (MB/s, empty for unlimited)
          <input
            type="number"
            min={0}
            className="input py-1.5"
            value={limitMb}
            onChange={(e) => setConfig({ ...config, bandwidthLimit: e.target.value ? Math.round(Number(e.target.value) * 1024 ** 2) : null })}
          />
        </label>
        <div className="flex items-center gap-3">
          <Wifi className="size-4 text-cyan-300" />
          <div className="flex-1">
            <Toggle
              label="Pause downloads on metered connections (hotspots, capped plans)"
              checked={config.pauseOnMetered}
              onChange={(pauseOnMetered) => setConfig({ ...config, pauseOnMetered })}
            />
          </div>
        </div>
      </section>
      {message && <div className="text-sm text-slate-400">{message}</div>}
    </div>
  );
}
