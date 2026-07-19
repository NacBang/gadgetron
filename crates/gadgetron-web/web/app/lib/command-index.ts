export interface CommandIndexEntry {
  id: string;
  label: string;
  description: string;
  keywords?: readonly string[];
}

function normalized(value: string): string {
  return value.trim().toLocaleLowerCase();
}

/**
 * Shared command lookup used by both the composer slash menu and the shell
 * command palette. Prefix matches stay first, then token-complete substring
 * matches retain their declared order.
 */
export function searchCommandIndex<T extends CommandIndexEntry>(
  entries: readonly T[],
  query: string,
): T[] {
  const needle = normalized(query);
  if (!needle) return [...entries];
  const tokens = needle.split(/\s+/).filter(Boolean);

  return entries
    .flatMap((entry, index) => {
      const fields = [entry.label, ...(entry.keywords ?? [])].map(normalized);
      const haystack = normalized(
        [entry.label, entry.description, ...(entry.keywords ?? [])].join(" "),
      );
      if (!tokens.every((token) => haystack.includes(token))) return [];
      const prefix = fields.some((field) => field.startsWith(needle));
      return [{ entry, index, rank: prefix ? 0 : 1 }];
    })
    .sort((left, right) => left.rank - right.rank || left.index - right.index)
    .map(({ entry }) => entry);
}
