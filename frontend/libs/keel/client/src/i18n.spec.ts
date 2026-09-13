import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { KeelI18n, provideKeelClient, type Catalogue } from '@globalart/keel/client';
import type { Envelope } from '@globalart/keel/meta';

const CATALOGUE: Catalogue = {
  lang: 'ru',
  messages: {
    keel: { name: 'Name', check: { ip: 'Enter an IP address' } },
    dns: { zone: { list: { title: 'Zones' }, error: { exists: 'Zone {name} exists already' } } },
  },
};

function served(catalogue: Catalogue): Envelope<Catalogue> {
  return { ok: true, data: catalogue, meta: { requestId: 'req-i18n' } };
}

describe('KeelI18n', () => {
  let i18n: KeelI18n;
  let http: HttpTestingController;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
    });
    i18n = TestBed.inject(KeelI18n);
    http = TestBed.inject(HttpTestingController);
  });

  afterEach(() => http.verify());

  it('t translates through the catalogue with the shared fallback and the key when nothing matches', async () => {
    expect(i18n.lang()).toBeNull();
    expect(i18n.t('dns.zone.list.title')).toBe('dns.zone.list.title');

    const loading = i18n.load('ru');
    const request = http.expectOne({ method: 'GET', url: '/api/v1/i18n?lang=ru' });
    expect(request.request.headers.has('If-None-Match')).toBe(false);
    request.flush(served(CATALOGUE), { headers: { ETag: '"c-1"' } });
    expect(await loading).toEqual(CATALOGUE);
    expect(i18n.lang()).toBe('ru');
    expect(i18n.t('dns.zone.list.title')).toBe('Zones');
    expect(i18n.t('dns.zone.error.exists', { name: 'a.org' })).toBe('Zone a.org exists already');
    expect(i18n.t('dns.zone.name')).toBe('Name');
    expect(i18n.t('keel.check.ip')).toBe('Enter an IP address');
    expect(i18n.t('dns.zone.missing')).toBe('dns.zone.missing');
  });

  it('a refetch of the same language sends the ETag and keeps the catalogue on 304', async () => {
    const first = i18n.load('ru');
    http.expectOne('/api/v1/i18n?lang=ru').flush(served(CATALOGUE), { headers: { ETag: '"c-1"' } });
    await first;

    const again = i18n.load('ru');
    const refetch = http.expectOne('/api/v1/i18n?lang=ru');
    expect(refetch.request.headers.get('If-None-Match')).toBe('"c-1"');
    refetch.flush(null, { status: 304, statusText: 'Not Modified' });
    expect(await again).toEqual(CATALOGUE);
    expect(i18n.t('dns.zone.list.title')).toBe('Zones');

    const other = i18n.load('en');
    const english = http.expectOne('/api/v1/i18n?lang=en');
    expect(english.request.headers.has('If-None-Match')).toBe(false);
    english.flush(
      served({ lang: 'en', messages: { dns: { zone: { list: { title: 'DNS zones' } } } } }),
    );
    await other;
    expect(i18n.lang()).toBe('en');
    expect(i18n.t('dns.zone.list.title')).toBe('DNS zones');
  });
});
