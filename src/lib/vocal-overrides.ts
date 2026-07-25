export type TrackOverrides = Record<number, boolean>;

export function applyTrackOverrides(
  current: TrackOverrides | undefined,
  trackIds: readonly number[],
  enabled: boolean,
): TrackOverrides {
  const next = { ...(current ?? {}) };
  for (const trackId of trackIds) {
    next[trackId] = enabled;
  }
  return next;
}
