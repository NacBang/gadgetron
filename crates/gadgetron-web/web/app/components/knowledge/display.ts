export function displayDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "Unknown"
    : new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(date);
}

export function displayBytes(value?: number) {
  if (value === undefined) return "—";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

export function noteTitle(path: string, properties?: Record<string, unknown>) {
  const title = properties?.title;
  if (typeof title === "string" && title.trim()) return title;
  const stem = path.split("/").pop()?.replace(/\.md$/i, "") || path;
  return stem
    .replaceAll(/[-_]+/g, " ")
    .replace(/\b\p{L}/gu, (letter) => letter.toLocaleUpperCase());
}

export function humanizeIdentifier(value: string) {
  return value
    .split(/[-_.]+/)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toLocaleUpperCase()}${part.slice(1)}`)
    .join(" ");
}

export function knowledgeSpaceTitle(value: string) {
  const title = value.replace(/^R\d+(?:\.\d+)*\s+/i, "").trim();
  if (title === "Domain Vault") return "Knowledge Lab";
  if (title === "Personal") return "Personal Knowledge";
  return title || value;
}
