// Client-side model of the organizer path pattern. Serializes to the same
// `{Token}/literal-{Token}` template string the Rust organizer parses.

export const TOKENS = ["Platform", "Author", "Year", "Month", "Day", "Date", "Resolution", "MediaType"] as const;
export type TokenName = (typeof TOKENS)[number];

export type Part = { kind: "token"; name: TokenName } | { kind: "literal"; value: string };
export type Segment = Part[];

const TOKEN_SET = new Set<string>(TOKENS);

export function parsePattern(template: string): Segment[] {
  return template
    .split(/[\\/]/)
    .filter((s) => s.trim())
    .map((seg) => {
      const parts: Part[] = [];
      const re = /[{[]([A-Za-z]+)[}\]]/g;
      let last = 0;
      for (const m of seg.matchAll(re)) {
        const name = m[1] ?? "";
        const canonical = TOKENS.find((t) => t.toLowerCase() === name.toLowerCase());
        if (m.index > last) parts.push({ kind: "literal", value: seg.slice(last, m.index) });
        parts.push(canonical ? { kind: "token", name: canonical } : { kind: "literal", value: m[0] });
        last = m.index + m[0].length;
      }
      if (last < seg.length) parts.push({ kind: "literal", value: seg.slice(last) });
      return parts;
    });
}

export function serializePattern(segments: Segment[]): string {
  return segments
    .filter((s) => s.length)
    .map((s) => s.map((p) => (p.kind === "token" ? `{${p.name}}` : p.value)).join(""))
    .join("/");
}

export function isToken(name: string): name is TokenName {
  return TOKEN_SET.has(name);
}
