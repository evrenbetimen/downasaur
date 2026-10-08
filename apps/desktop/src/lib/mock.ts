// Browser-only mock backend used when the UI runs outside Tauri.
import { detectPlatform, PLATFORMS } from "./platforms";
import type {
  DownloadProfile,
  DownloadTask,
  Extraction,
  MediaInfo,
  OrganizerRules,
  Platform,
  ProgressEvent,
  SchedulerConfig,
  SniffResult,
  StreamFormat,
} from "./types";

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
const uuid = () => crypto.randomUUID();

const fmt = (id: string, kind: StreamFormat["kind"], height: number | null, codec: StreamFormat["videoCodec"]): StreamFormat => ({
  id,
  url: `https://cdn.example/${id}`,
  kind,
  transport: { type: "http", contentLength: null },
  container: "webm",
  width: height ? Math.round((height * 16) / 9) : null,
  height,
  fps: height ? 60 : null,
  videoCodec: codec,
  audioCodec: kind === "audioOnly" ? "opus" : null,
  bitrate: null,
  hdr: height === 4320,
});

const SAMPLES: Record<Platform, Omit<MediaInfo, "sourceUrl">> = {
  youtube: {
    platform: "youtube",
    id: "8k-demo",
    title: "🔥 iPhone 18 Review: SHOCKING!!! #apple",
    author: "MKBHD",
    thumbnail: null,
    durationSecs: 1124,
    publishedAt: "2027-03-09T12:00:00Z",
    contentKind: "video",
    formats: [
      fmt("571", "videoOnly", 4320, "av1"),
      fmt("313", "videoOnly", 2160, "vp9"),
      fmt("137", "videoOnly", 1080, "h264"),
      fmt("251", "audioOnly", null, null),
    ],
    subtitles: [],
  },
  tiktok: {
    platform: "tiktok",
    id: "7300000000000000000",
    title: "sunset skate session #fyp",
    author: "skater",
    thumbnail: null,
    durationSecs: 21,
    publishedAt: null,
    contentKind: "short",
    formats: [fmt("play", "muxed", 1920, "hevc")],
    subtitles: [],
  },
  instagram: { platform: "instagram", id: "Cxyz", title: "Reel", author: "creator", thumbnail: null, durationSecs: 14, publishedAt: null, contentKind: "short", formats: [fmt("0", "muxed", 1920, "h264")], subtitles: [] },
  twitch: { platform: "twitch", id: "123", title: "Twitch VOD 123", author: "streamer", thumbnail: null, durationSecs: 7200, publishedAt: null, contentKind: "vod", formats: [fmt("chunked", "muxed", 1080, "h264")], subtitles: [] },
  twitter: { platform: "twitter", id: "1", title: "Launch day 🚀", author: "acme", thumbnail: null, durationSecs: 30, publishedAt: null, contentKind: "video", formats: [fmt("mp4-10368000", "muxed", 1080, "h264")], subtitles: [] },
  facebook: { platform: "facebook", id: "1", title: "Trip", author: "Ana", thumbnail: null, durationSecs: 95, publishedAt: null, contentKind: "video", formats: [fmt("hd", "muxed", 720, "h264")], subtitles: [] },
  generic: { platform: "generic", id: "1", title: "The New Vimeo Player", author: "Vimeo", thumbnail: null, durationSecs: 62, publishedAt: null, contentKind: "video", formats: [fmt("hls-1080p", "muxed", 1080, "h264")], subtitles: [] },
};

type Listener = (batch: ProgressEvent[]) => void;
const listeners = new Set<Listener>();
const tasks = new Map<string, DownloadTask & { speed: number }>();
let rules: OrganizerRules = {
  enabled: true,
  targetDir: "~/Videos/Downasaur",
  pattern: "{Platform}/{Author}/{Year}-{Month}",
  separation: { enabled: true, audioDir: "Music", eightKDir: "Videos/8K", shortsDir: "Shorts" },
  cleanNames: true,
};
let scheduler: SchedulerConfig = {
  enabled: false,
  windows: [{ start: "02:00:00", end: "06:00:00", days: [] }],
  bandwidthLimit: null,
  pauseOnMetered: true,
};

function emit(batch: ProgressEvent[]) {
  if (batch.length) listeners.forEach((l) => l(batch));
}

setInterval(() => {
  const batch: ProgressEvent[] = [];
  for (const t of tasks.values()) {
    if (t.state === "downloading") {
      const total = t.bytesTotal ?? 1;
      t.bytesDone = Math.min(total, t.bytesDone + t.speed / 10);
      batch.push({ kind: "download", taskId: t.id, bytesDone: t.bytesDone, bytesTotal: total, speedBps: t.speed, etaSecs: (total - t.bytesDone) / t.speed });
      if (t.bytesDone >= total) {
        t.state = "muxing";
        batch.push({ kind: "state", taskId: t.id, state: "muxing", message: null });
      }
    } else if (t.state === "muxing") {
      t.state = "completed";
      t.destination = `${rules.targetDir}/${PLATFORMS[t.platform].label}/${t.author ?? "Unknown"}/clean_name.mkv`;
      batch.push({ kind: "muxing", taskId: t.id, percent: 100, label: "Muxing 8K Video..." });
      batch.push({ kind: "organized", taskId: t.id, destination: t.destination });
      batch.push({ kind: "state", taskId: t.id, state: "completed", message: null });
    }
  }
  emit(batch);
}, 100);

export const mock = {
  async sniffLink(input: string): Promise<SniffResult> {
    const platform = detectPlatform(input);
    if (!platform) throw { code: "invalid_url", message: "That doesn't look like a link." };
    return { platform, displayName: PLATFORMS[platform].label };
  },
  async fetchMetadata(input: string): Promise<Extraction> {
    await delay(450);
    const platform = detectPlatform(input) ?? "generic";
    if (/netflix|disneyplus|primevideo/.test(input)) {
      throw { code: "drm_protected", message: "DRM Protected Content: this media is protected by Widevine and cannot be downloaded." };
    }
    if (/[?&]list=|\/playlist|youtube\.com\/@/.test(input)) {
      return { type: "collection", platform, title: "Mock playlist", entries: Array.from({ length: 12 }, (_, i) => `${input}#${i}`), continuation: null };
    }
    return { type: "media", ...SAMPLES[platform], sourceUrl: input };
  },
  async expandCollection(entries: string[]): Promise<MediaInfo[]> {
    await delay(300);
    return entries.map((u, i) => ({ ...SAMPLES.youtube, id: `e${i}`, title: `Playlist entry ${i + 1}`, sourceUrl: u }));
  },
  async enqueue(items: MediaInfo[], profile: DownloadProfile): Promise<DownloadTask[]> {
    const now = new Date().toISOString();
    return items.map((info) => {
      const t = {
        id: uuid(),
        sourceUrl: info.sourceUrl,
        platform: info.platform,
        title: info.title,
        author: info.author,
        profile,
        state: "downloading" as const,
        bytesTotal: (profile.type === "video" && profile.maxResolution === "4320p" ? 6 : 0.4) * 1024 ** 3,
        bytesDone: 0,
        destination: null,
        error: null,
        createdAt: now,
        updatedAt: now,
        speed: (40 + Math.random() * 160) * 1024 ** 2,
      };
      tasks.set(t.id, t);
      return t;
    });
  },
  async list(): Promise<DownloadTask[]> {
    return [...tasks.values()];
  },
  async pause(id: string) {
    const t = tasks.get(id);
    if (t) {
      t.state = "paused";
      emit([{ kind: "state", taskId: id, state: "paused", message: null }]);
    }
  },
  async resume(id: string) {
    const t = tasks.get(id);
    if (t) {
      t.state = "downloading";
      emit([{ kind: "state", taskId: id, state: "downloading", message: null }]);
    }
  },
  async cancel(id: string) {
    const t = tasks.get(id);
    if (t) {
      t.state = "cancelled";
      emit([{ kind: "state", taskId: id, state: "cancelled", message: null }]);
    }
  },
  async getRules() {
    return structuredClone(rules);
  },
  async setRules(next: OrganizerRules) {
    rules = structuredClone(next);
  },
  async previewPath(r: OrganizerRules): Promise<string> {
    const values: Record<string, string> = {
      platform: "YouTube", author: "MKBHD", year: "2027", month: "03", day: "09", date: "2027-03-09", resolution: "4320p", mediatype: "Video",
    };
    const rendered = r.pattern.replace(/[{[]([A-Za-z]+)[}\]]/g, (m, name: string) => values[name.toLowerCase()] ?? m);
    const media = r.separation.enabled ? `/${r.separation.eightKDir}` : "";
    const name = r.cleanNames ? "MKBHD_iPhone_18_Review" : "iPhone 18 Review_ SHOCKING!!! #apple";
    return `${r.targetDir}${media}/${rendered}/${name}.mkv`;
  },
  async getScheduler() {
    return structuredClone(scheduler);
  },
  async setScheduler(next: SchedulerConfig) {
    scheduler = structuredClone(next);
  },
  subscribe(l: Listener) {
    listeners.add(l);
    return () => void listeners.delete(l);
  },
};
