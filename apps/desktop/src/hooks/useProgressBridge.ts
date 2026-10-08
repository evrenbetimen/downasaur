import { useEffect } from "react";
import { api, onProgress } from "../lib/api";
import { queueStore } from "../lib/queueStore";

/** Load the persisted queue once and keep it live from the 60 fps progress event. */
export function useProgressBridge() {
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    api.listDownloads().then((t) => !disposed && queueStore.replaceAll(t)).catch(console.error);
    onProgress((batch) => queueStore.apply(batch)).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
