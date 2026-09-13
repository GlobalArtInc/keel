import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import {
  KeelBoot,
  KeelCallError,
  KeelClient,
  KeelDescriptors,
  provideKeelClient,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const BOOT = load<Boot>('boot.json');
const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const OK = load<Envelope>('envelope-200.json');

describe('KeelDescriptors', () => {
  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
    });
  });

  afterEach(() => TestBed.inject(HttpTestingController).verify());

  async function booted(): Promise<HttpTestingController> {
    const http = TestBed.inject(HttpTestingController);
    const loading = TestBed.inject(KeelBoot).load();
    http.expectOne('/api/v1/boot').flush(BOOT, { headers: { ETag: `"${BOOT.meta}"` } });
    await loading;
    return http;
  }

  it('descriptors are cached under the meta version and dropped when it changes', async () => {
    const http = await booted();
    const descriptors = TestBed.inject(KeelDescriptors);

    const first = descriptors.get('dns.zone.list');
    const second = descriptors.get('dns.zone.list');
    http
      .expectOne({ method: 'GET', url: '/api/v1/functions/dns.zone.list' })
      .flush(LIST, { headers: { ETag: `"${BOOT.meta}"` } });
    expect(await first).toEqual(LIST);
    expect(await second).toEqual(LIST);
    expect(await descriptors.get('dns.zone.list')).toEqual(LIST);
    http.expectNone('/api/v1/functions/dns.zone.list');

    const changing = firstValueFrom(TestBed.inject(KeelClient).call('dns.zone.edit', {}));
    http
      .expectOne('/api/v1/call/dns.zone.edit')
      .flush(OK, { headers: { 'X-Keel-Meta-Version': 'm-after' } });
    await changing;

    const third = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    expect(await third).toEqual(LIST);
  });

  it('the index fills the cache and a failed fetch is not kept', async () => {
    const http = await booted();
    const descriptors = TestBed.inject(KeelDescriptors);

    const index = descriptors.list();
    http.expectOne({ method: 'GET', url: '/api/v1/functions' }).flush([LIST, EDIT]);
    expect(await index).toEqual([LIST, EDIT]);
    expect(await descriptors.get('dns.zone.edit')).toEqual(EDIT);
    http.expectNone('/api/v1/functions/dns.zone.edit');

    const missing = descriptors.get('dns.zone.gone');
    http
      .expectOne('/api/v1/functions/dns.zone.gone')
      .flush(
        { ok: false, error: { code: 'not_found', key: 'keel.error.function_not_found' } },
        { status: 404, statusText: 'Not Found' },
      );
    await expect(missing).rejects.toBeInstanceOf(KeelCallError);
    await expect(missing).rejects.toMatchObject({ error: { code: 'not_found' } });

    const retried = descriptors.get('dns.zone.gone');
    http.expectOne('/api/v1/functions/dns.zone.gone').flush(EDIT);
    expect(await retried).toEqual(EDIT);
  });
});
