import { useState } from "react";
import { AppShell, type View } from "./components/AppShell";
import { useProgressBridge } from "./hooks/useProgressBridge";
import { LinkSniffer } from "./features/sniffer/LinkSniffer";
import { QueueTable } from "./features/queue/QueueTable";
import { QueueStats } from "./features/queue/QueueStats";
import { OrganizerRulesPanel } from "./features/organizer/OrganizerRulesPanel";
import { SchedulerPanel } from "./features/scheduler/SchedulerPanel";

export default function App() {
  const [view, setView] = useState<View>("downloads");
  useProgressBridge();

  return (
    <AppShell view={view} onNavigate={setView}>
      {view === "downloads" && (
        <div className="flex h-full min-h-0 flex-col gap-4">
          <LinkSniffer />
          <QueueStats />
          <QueueTable />
        </div>
      )}
      {view === "organizer" && <OrganizerRulesPanel />}
      {view === "scheduler" && <SchedulerPanel />}
    </AppShell>
  );
}
