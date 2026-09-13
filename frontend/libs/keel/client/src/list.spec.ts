import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { KeelCallError, listFetch, provideKeelClient, toListQuery } from '@globalart/keel/client';
import type { Envelope, ListQuery, Page } from '@globalart/keel/meta';
import type { PhxDataTableQuery } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST_QUERY = load<ListQuery>('list-query.json');
const PAGE = load<Page>('page.json');

const TABLE_QUERY: PhxDataTableQuery = {
  page: 2,
  pageSize: 25,
  offset: 25,
  limit: 25,
  search: 'www',
  sort: { key: 'name', dir: 'asc' },
  filters: [{ key: 'type', value: ['A', 'AAAA'] }],
};

function expected(): ListQuery {
  return { ...LIST_QUERY, filters: (LIST_QUERY.filters ?? []).filter((filter) => !filter.op) };
}

describe('listFetch', () => {
  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
    });
  });

  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('the adapter maps a table query onto the list query and the page back', async () => {
    const http = TestBed.inject(HttpTestingController);
    const fetch = TestBed.runInInjectionContext(() =>
      listFetch<{ id: string }>('dns.record.list', LIST_QUERY.scope),
    );
    expect(fetch.totals()).toBeNull();

    const pending = firstValueFrom(fetch(TABLE_QUERY));
    const request = http.expectOne({ method: 'POST', url: '/api/v1/call/dns.record.list' });
    expect(request.request.body).toEqual(expected());
    expect(request.request.body).not.toHaveProperty('offset');
    expect(request.request.body).not.toHaveProperty('limit');
    const envelope: Envelope<Page> = {
      ok: true,
      data: { ...PAGE, totals: { records: 12 } },
      meta: { requestId: 'req-list' },
    };
    request.flush(envelope);
    expect(await pending).toEqual({ data: PAGE.data, total: PAGE.total });
    expect(fetch.totals()).toEqual({ records: 12 });

    const again = firstValueFrom(fetch(TABLE_QUERY));
    http.expectOne('/api/v1/call/dns.record.list').flush({ ok: true, data: PAGE, meta: {} });
    expect(await again).toEqual({ data: PAGE.data, total: PAGE.total });
    expect(fetch.totals()).toBeNull();
  });

  it('an empty search, sort, filters and scope are left out of the body', () => {
    const query: PhxDataTableQuery = {
      page: 1,
      pageSize: 50,
      offset: 0,
      limit: 50,
      search: '',
      sort: null,
      filters: [],
    };
    expect(toListQuery(query)).toEqual({ page: 1, pageSize: 50 });
    expect(toListQuery(query, {})).toEqual({ page: 1, pageSize: 50 });
  });

  it('an error envelope fails the fetch with the call error', async () => {
    const http = TestBed.inject(HttpTestingController);
    const fetch = TestBed.runInInjectionContext(() => listFetch('dns.record.list'));
    const pending = firstValueFrom(fetch(TABLE_QUERY));
    http.expectOne('/api/v1/call/dns.record.list').flush(load('envelope-403.json'), {
      status: 403,
      statusText: 'Forbidden',
    });
    await expect(pending).rejects.toBeInstanceOf(KeelCallError);
    await expect(pending).rejects.toMatchObject({ error: { code: 'forbidden_field' } });
  });
});
