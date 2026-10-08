import { Activity, CheckCircle2, Clock4, Zap } from "lucide-react";
import { formatSpeed } from "../../lib/format";
import { useQueue } from "../../lib/queueStore";

const ACTIVE = new Set(["fetching", "downloading", "muxing", "organizing"]);

export function QueueStats() {
  const rows = useQueue();
  const active = rows.filter((r) => ACTIVE.has(r.state)).length;
  const waiting = rows.filter((r) => r.state === "queued" || r.state === "scheduled" || r.state === "paused").length;
  const done = rows.filter((r) => r.state === "completed").length;
  const speed = rows.reduce((s, r) => s + (r.state === "downloading" ? r.speedBps : 0), 0);

  const stats = [
    { label: "Active", value: active, icon: Activity, tint: "text-cyan-300" },
    { label: "Waiting", value: waiting, icon: Clock4, tint: "text-amber-300" },
    { label: "Completed", value: done, icon: CheckCircle2, tint: "text-emerald-300" },
    { label: "Throughput", value: formatSpeed(speed), icon: Zap, tint: "text-fuchsia-300" },
  ];

  return (
    <div className="grid grid-cols-4 gap-3">
      {stats.map(({ label, value, icon: Icon, tint }) => (
        <div key={label} className="panel flex items-center gap-3 px-4 py-3">
          <Icon className={`size-4 ${tint}`} />
          <div>
            <div className="text-[11px] uppercase tracking-wider text-slate-500">{label}</div>
            <div className="font-mono text-lg font-semibold tabular-nums text-white">{value}</div>
          </div>
        </div>
      ))}
    </div>
  );
}
