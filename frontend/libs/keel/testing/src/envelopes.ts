import type { CallError, Envelope, Page } from '@globalart/keel/meta';

export const STORY_REQUEST_ID = 'req-story';

export function okEnvelope<T>(data: T, requestId = STORY_REQUEST_ID): Envelope<T> {
  return { ok: true, data, meta: { requestId } };
}

export function pageEnvelope<Row>(
  rows: readonly Row[],
  totals: Record<string, unknown> | null = null,
): Envelope<Page<Row>> {
  return okEnvelope({ data: [...rows], total: rows.length, totals });
}

export function errorEnvelope(error: CallError, requestId = STORY_REQUEST_ID): Envelope<never> {
  return { ok: false, error: { requestId, ...error } };
}
