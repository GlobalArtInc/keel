import type { Event as KeelEvent } from '@globalart/keel/meta';
import { filter, type Observable } from 'rxjs';

export const MASK_SUFFIX = '.*';

export function maskMatches(mask: string, name: string): boolean {
  if (!mask.endsWith(MASK_SUFFIX)) {
    return mask === name;
  }
  const prefix = mask.slice(0, -MASK_SUFFIX.length);
  return name.startsWith(`${prefix}.`) && name.length > prefix.length + 1;
}

export function matchesAny(masks: readonly string[], name: string): boolean {
  return masks.some((mask) => maskMatches(mask, name));
}

export function invalidations(
  events: Observable<KeelEvent>,
  masks: () => readonly string[],
): Observable<KeelEvent> {
  return events.pipe(filter((event) => matchesAny(masks(), event.type)));
}
