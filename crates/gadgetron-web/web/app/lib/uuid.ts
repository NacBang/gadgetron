// `crypto.randomUUID()` is only defined in secure contexts (HTTPS or
// `localhost`). Gadgetron is designed to be reachable on plain-HTTP LAN
// addresses like `10.100.1.5:18080`, where `crypto.randomUUID` is
// undefined — calling it crashes with `TypeError: crypto.randomUUID is
// not a function`. Observed 2026-04-20 on `/web/wiki` (operator bug
// report, ROADMAP ISSUE 29 TASK 29.5).
//
// Fall back to a v4-shaped string assembled from `Math.random()`. Not
// cryptographically strong — we only need uniqueness within a single
// tab's request history (the `client_invocation_id` dedupe window), not
// unguessability.
export function safeRandomUUID(): string {
  const g: Crypto | undefined =
    typeof crypto !== "undefined" ? crypto : undefined;
  if (g && typeof g.randomUUID === "function") {
    return g.randomUUID();
  }
  const bytes = new Uint8Array(16);
  if (g && typeof g.getRandomValues === "function") {
    g.getRandomValues(bytes);
  } else {
    for (let i = 0; i < 16; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex: string[] = [];
  for (let i = 0; i < 16; i++) hex.push(bytes[i].toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex
    .slice(6, 8)
    .join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10, 16).join("")}`;
}
