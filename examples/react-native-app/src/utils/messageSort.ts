/**
 * Comparator for causal message ordering using Lamport clocks.
 *
 * Rules:
 * - Legacy messages (lamportClock === 0) sort by wall-clock timestamp.
 * - Legacy messages always sort before Lamport-bearing messages
 *   (they predate Lamport support).
 * - Among Lamport-bearing messages, sort by clock value ascending.
 * - Ties are broken by message ID for deterministic stability.
 */

interface Sortable {
  id: string;
  lamportClock: number;
  timestamp: number;
}

export function compareByCausalOrder(a: Sortable, b: Sortable): number {
  const aLegacy = a.lamportClock === 0;
  const bLegacy = b.lamportClock === 0;

  if (aLegacy && bLegacy) return a.timestamp - b.timestamp;
  if (aLegacy !== bLegacy) return aLegacy ? -1 : 1;

  const clockDiff = a.lamportClock - b.lamportClock;
  if (clockDiff !== 0) return clockDiff;

  return a.id.localeCompare(b.id);
}
