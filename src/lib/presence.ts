import type { PresenceEvent, PresenceInterval } from "../types";

/** Collapse join/leave events into presence intervals per participant. */
export function intervalsFromEvents(
  events: PresenceEvent[],
  durationMs: number,
): PresenceInterval[] {
  const byId = new Map<string, PresenceEvent[]>();
  for (const ev of events) {
    const list = byId.get(ev.participant_id) ?? [];
    list.push(ev);
    byId.set(ev.participant_id, list);
  }
  const out: PresenceInterval[] = [];
  for (const [participant_id, list] of byId) {
    const sorted = [...list].sort((a, b) => a.t_ms - b.t_ms);
    let open: number | null = null;
    for (const ev of sorted) {
      if (ev.kind === "join") {
        if (open == null) open = Math.max(0, ev.t_ms);
      } else if (ev.kind === "leave") {
        if (open != null) {
          out.push({
            participant_id,
            start_ms: open,
            end_ms: Math.max(open, ev.t_ms),
          });
          open = null;
        }
      }
    }
    if (open != null) {
      out.push({
        participant_id,
        start_ms: open,
        end_ms: durationMs > 0 ? durationMs : null,
      });
    }
  }
  return out;
}

export function isPresentAt(
  intervals: PresenceInterval[],
  participantId: string,
  tMs: number,
): boolean {
  return intervals.some(
    (iv) =>
      iv.participant_id === participantId &&
      tMs >= iv.start_ms &&
      (iv.end_ms == null || tMs < iv.end_ms),
  );
}

/** First join time per participant — used as A/V pad offset. */
export function firstJoinOffsetsMs(
  events: PresenceEvent[],
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const ev of events) {
    if (ev.kind !== "join") continue;
    const prev = out[ev.participant_id];
    if (prev == null || ev.t_ms < prev) out[ev.participant_id] = ev.t_ms;
  }
  return out;
}
