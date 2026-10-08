import { PLATFORMS } from "../lib/platforms";
import type { Platform } from "../lib/types";

export function PlatformBadge({ platform, size = "md" }: { platform: Platform; size?: "sm" | "md" | "lg" }) {
  const meta = PLATFORMS[platform];
  const dims = { sm: "size-6 text-[11px] rounded-md", md: "size-8 text-sm rounded-lg", lg: "size-12 text-xl rounded-2xl" }[size];
  return (
    <span title={meta.label} aria-label={meta.label} className={`inline-grid shrink-0 place-items-center ${dims} ${meta.badge}`}>
      {meta.glyph}
    </span>
  );
}
