import type { Scope } from '@globalart/keel/client';

export const SEARCH_PARAM = 'search';

export function searchOf(params: Record<string, unknown>): string {
  const search = params[SEARCH_PARAM];
  return typeof search === 'string' ? search : '';
}

export function scopeOf(params: Record<string, unknown>): Scope {
  const scope: Scope = {};
  for (const [name, value] of Object.entries(params)) {
    if (name !== SEARCH_PARAM) {
      scope[name] = value;
    }
  }
  return scope;
}
