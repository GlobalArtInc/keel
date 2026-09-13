import { inject, signal, type Signal } from '@angular/core';
import type { ListQuery, Page } from '@globalart/keel/meta';
import type { PhxDataTableFetch, PhxDataTableQuery } from '@globalart/phoenix-ui';
import { map } from 'rxjs';
import { KeelCallError, KeelClient } from './client';

export type Totals = Record<string, unknown>;

export type Scope = Record<string, unknown>;

export type KeelListFetch<Row> = PhxDataTableFetch<Row> & { totals: Signal<Totals | null> };

export function toListQuery(query: PhxDataTableQuery, scope?: Scope): ListQuery {
  const body: ListQuery = { page: query.page, pageSize: query.pageSize };
  if (query.search !== '') {
    body.search = query.search;
  }
  if (query.sort !== null) {
    body.sort = { key: query.sort.key, dir: query.sort.dir };
  }
  if (query.filters.length > 0) {
    body.filters = query.filters.map(({ key, value }) => ({ key, value }));
  }
  if (scope !== undefined && Object.keys(scope).length > 0) {
    body.scope = scope;
  }
  return body;
}

export function listFetch<Row>(
  func: string,
  scope?: Scope,
  client: KeelClient = inject(KeelClient),
): KeelListFetch<Row> {
  const totals = signal<Totals | null>(null);
  const fetch: PhxDataTableFetch<Row> = (query) =>
    client.call<Page<Row>>(func, toListQuery(query, scope)).pipe(
      map((envelope) => {
        if (!envelope.ok) {
          throw new KeelCallError(envelope.error);
        }
        totals.set(envelope.data.totals ?? null);
        return { data: envelope.data.data, total: envelope.data.total };
      }),
    );
  return Object.assign(fetch, { totals: totals.asReadonly() });
}
