import type { ReactNode } from "react";
import { CalendarClock, Download, FolderTree, Gauge } from "lucide-react";
import { isTauri } from "../lib/api";

export type View = "downloads" | "organizer" | "scheduler";

const NAV: { id: View; label: string; icon: typeof Download; hint: string }[] = [
  { id: "downloads", label: "Downloads", icon: Download, hint: "Paste, pick, queue" },
  { id: "organizer", label: "Organizer", icon: FolderTree, hint: "Folder rules & naming" },
  { id: "scheduler", label: "Scheduler", icon: CalendarClock, hint: "Windows & network guard" },
];

export function AppShell({ view, onNavigate, children }: { view: View; onNavigate: (v: View) => void; children: ReactNode }) {
  return (
    <div className="grid h-full grid-cols-[232px_1fr] bg-[radial-gradient(1200px_600px_at_80%_-10%,rgba(34,211,238,0.08),transparent),radial-gradient(900px_500px_at_-10%_110%,rgba(52,211,153,0.07),transparent)]">
      <aside className="flex flex-col border-r border-white/[0.06] bg-ink-900/60 px-3 py-5 backdrop-blur">
        <div className="mb-8 flex items-center gap-2.5 px-2">
          <img src="/downasaur.svg" alt="" className="size-9" />
          <div>
            <div className="text-[15px] font-semibold tracking-tight text-white">Downasaur</div>
            <div className="bg-gradient-to-r from-emerald-300 to-cyan-300 bg-clip-text text-[11px] font-bold tracking-[0.2em] text-transparent">
              8K ENGINE
            </div>
          </div>
        </div>

        <nav className="flex flex-col gap-1">
          {NAV.map(({ id, label, icon: Icon, hint }) => (
            <button
              key={id}
              onClick={() => onNavigate(id)}
              className={`group flex items-start gap-3 rounded-xl px-3 py-2.5 text-left transition ${
                view === id ? "bg-white/[0.07] text-white" : "text-slate-400 hover:bg-white/[0.04] hover:text-slate-200"
              }`}
            >
              <Icon className={`mt-0.5 size-4 ${view === id ? "text-emerald-300" : ""}`} />
              <span>
                <span className="block text-sm font-medium">{label}</span>
                <span className="block text-[11px] text-slate-500">{hint}</span>
              </span>
            </button>
          ))}
        </nav>

        <div className="mt-auto rounded-xl border border-white/[0.06] bg-ink-850/80 p-3 text-[11px] text-slate-400">
          <div className="mb-1 flex items-center gap-1.5 font-medium text-slate-300">
            <Gauge className="size-3.5 text-cyan-300" /> Engine
          </div>
          {isTauri ? "Native Rust core connected" : "Browser preview · mock backend"}
        </div>
      </aside>

      <main className="min-h-0 overflow-hidden p-6">{children}</main>
    </div>
  );
}
