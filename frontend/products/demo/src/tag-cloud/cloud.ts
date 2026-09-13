import { KeelCallError, type KeelClient } from '@globalart/keel/client';
import type { ListQuery, Page } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';

export const TAG_LIST = 'note.tag.list';
export const NOTE_LIST = 'note.list';
export const TAG_EMPTY_KEY = 'note.tag.empty';
export const CLOUD_PAGE_SIZE = 200;
export const MIN_SIZE = 14;
export const MAX_SIZE = 32;

export interface TagCount {
  tag: string;
  count: number;
}

export interface SizedTag extends TagCount {
  size: number;
}

export function isTagCount(row: unknown): row is TagCount {
  return (
    typeof row === 'object' &&
    row !== null &&
    typeof (row as { tag?: unknown }).tag === 'string' &&
    typeof (row as { count?: unknown }).count === 'number'
  );
}

export async function loadTags(
  client: KeelClient,
  scope: Record<string, unknown> = {},
  func = TAG_LIST,
): Promise<TagCount[]> {
  const rows: TagCount[] = [];
  for (let page = 1; ; page++) {
    const query: ListQuery = { page, pageSize: CLOUD_PAGE_SIZE };
    if (Object.keys(scope).length > 0) {
      query.scope = scope;
    }
    const envelope = await firstValueFrom(client.call<Page>(func, query));
    if (!envelope.ok) {
      throw new KeelCallError(envelope.error);
    }
    const data = envelope.data.data.filter(isTagCount);
    rows.push(...data);
    if (data.length === 0 || rows.length >= envelope.data.total) {
      return rows;
    }
  }
}

export function sizeOf(count: number, low: number, high: number): number {
  if (high <= low) {
    return Math.round((MIN_SIZE + MAX_SIZE) / 2);
  }
  const ratio = Math.min(1, Math.max(0, (count - low) / (high - low)));
  return Math.round(MIN_SIZE + ratio * (MAX_SIZE - MIN_SIZE));
}

export function sized(rows: readonly TagCount[]): SizedTag[] {
  if (rows.length === 0) {
    return [];
  }
  const counts = rows.map((row) => row.count);
  const low = Math.min(...counts);
  const high = Math.max(...counts);
  return rows.map((row) => ({ ...row, size: sizeOf(row.count, low, high) }));
}
