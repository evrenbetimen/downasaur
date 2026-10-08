import type { Platform } from "./types";

export interface PlatformMeta {
  label: string;
  /** Short glyph shown in the badge. */
  glyph: string;
  /** Tailwind classes for the badge. */
  badge: string;
}

export const PLATFORMS: Record<Platform, PlatformMeta> = {
  youtube: { label: "YouTube", glyph: "▶", badge: "bg-red-600 text-white" },
  tiktok: { label: "TikTok", glyph: "♪", badge: "bg-zinc-950 text-cyan-300 ring-1 ring-pink-500/70" },
  instagram: { label: "Instagram", glyph: "◎", badge: "bg-gradient-to-br from-amber-400 via-pink-500 to-violet-600 text-white" },
  twitch: { label: "Twitch", glyph: "⌘", badge: "bg-violet-600 text-white" },
  twitter: { label: "X", glyph: "𝕏", badge: "bg-black text-white ring-1 ring-white/20" },
  facebook: { label: "Facebook", glyph: "f", badge: "bg-blue-600 text-white font-bold" },
  generic: { label: "Web", glyph: "⌁", badge: "bg-slate-600 text-white" },
};

const HOSTS: [RegExp, Platform][] = [
  [/(^|\.)(youtube\.com|youtu\.be|youtube-nocookie\.com)$/i, "youtube"],
  [/(^|\.)tiktok\.com$/i, "tiktok"],
  [/(^|\.)(instagram\.com|instagr\.am)$/i, "instagram"],
  [/(^|\.)twitch\.tv$/i, "twitch"],
  [/(^|\.)(x\.com|twitter\.com)$/i, "twitter"],
  [/(^|\.)(facebook\.com|fb\.watch)$/i, "facebook"],
];

/**
 * Instant client-side detection so the icon appears while typing. The
 * authoritative answer comes from the Rust `sniff_link` command.
 */
export function detectPlatform(input: string): Platform | null {
  const text = input.trim();
  if (!text) return null;
  try {
    const url = new URL(text.includes("://") ? text : `https://${text}`);
    if (!/^https?:$/.test(url.protocol) || !url.hostname.includes(".")) return null;
    return HOSTS.find(([re]) => re.test(url.hostname))?.[1] ?? "generic";
  } catch {
    return null;
  }
}
