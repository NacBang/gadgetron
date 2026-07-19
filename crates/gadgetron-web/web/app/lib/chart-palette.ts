export const SERIES_PALETTE = [
  "#6F918E",
  "#7885A3",
  "#7E9272",
  "#927C96",
  "#698699",
  "#918975",
] as const;

export function seriesColor(index: number): string {
  return SERIES_PALETTE[index % SERIES_PALETTE.length];
}
