// Numbers and durations, spelled the way a person reads them.

const grouping = new Intl.NumberFormat("en-US");
const compacting = new Intl.NumberFormat("en-US", { notation: "compact", maximumSignificantDigits: 2 });

/** Group the digits of a count in threes: 1234567 becomes 1,234,567. */
export const grouped = (count) => grouping.format(count);

/** Spell a count in a few characters: 950, 12K, 4.1M, 600M. */
export const compact = (count) => compacting.format(count);

/** Spell a duration: 164 ms, 3.9 s, 1 m 36 s. */
export function duration(milliseconds) {
  if (milliseconds < 1000) return `${milliseconds} ms`;
  if (milliseconds < 60_000) return `${(milliseconds / 1000).toFixed(1)} s`;
  return `${Math.floor(milliseconds / 60_000)} m ${Math.floor((milliseconds % 60_000) / 1000)} s`;
}

/** Spell a share of a whole as a percentage; nothing when the whole is empty. */
export const percent = (part, whole) => (whole === 0 ? "" : `${((part * 100) / whole).toFixed(1)}%`);
