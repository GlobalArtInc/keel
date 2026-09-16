import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { KeelBoot, KeelClient, provideKeelClient, type CallOptions } from '@globalart/keel/client';
import type { Boot, Envelope } from '@globalart/keel/meta';
import { PhxGlobalProgressService } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const BOOT = load<Boot>('boot.json');
const OK = load<Envelope>('envelope-200.json');
const UNAUTHENTICATED: Envelope = {
  ok: false,
  error: { code: 'unauthenticated', key: 'keel.error.unauthenticated', requestId: 'req-401' },
};
const CROSS_SITE: Envelope = {
  ok: false,
  error: { code: 'forbidden', key: 'keel.error.cross_site', requestId: 'req-403' },
};
const WRONG_CODE: Envelope = {
  ok: false,
  error: { code: 'unauthenticated', key: 'auth.error.code', requestId: 'req-code' },
};

interface Harness {
  client: KeelClient;
  http: HttpTestingController;
  progress: PhxGlobalProgressService;
  metaChanges: string[];
  boot(): Promise<void>;
}

function harness(): Harness {
  TestBed.configureTestingModule({
    providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
  });
  const client = TestBed.inject(KeelClient);
  const http = TestBed.inject(HttpTestingController);
  const state: Harness = {
    client,
    http,
    progress: TestBed.inject(PhxGlobalProgressService),
    metaChanges: [],
    async boot() {
      const loading = TestBed.inject(KeelBoot).load();
      http
        .expectOne({ method: 'GET', url: '/api/v1/boot' })
        .flush(BOOT, { headers: { ETag: `"${BOOT.meta}"` } });
      await loading;
    },
  };
  client.metaChanged.subscribe((version) => state.metaChanges.push(version));
  return state;
}

function call(client: KeelClient, func: string, params: unknown, options?: CallOptions) {
  return firstValueFrom(client.call(func, params, options));
}

describe('KeelHttpClient', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('call posts the body, keeps the request id and the meta version, and a changed version refetches boot', async () => {
    const { client, http, metaChanges, boot } = harness();
    await boot();
    expect(client.metaVersion()).toBe(BOOT.meta);

    const params = { id: '01J9Q3W6B1R8N5X2K4T7M0V9Z3', ttl: 300 };
    const answer = call(client, 'dns.zone.edit', params);
    const request = http.expectOne({ method: 'POST', url: '/api/v1/call/dns.zone.edit' });
    expect(request.request.body).toEqual(params);
    expect(request.request.headers.has('Authorization')).toBe(false);
    request.flush(OK, {
      headers: { 'X-Keel-Request-Id': 'req-1a2b', 'X-Keel-Meta-Version': 'm-changed' },
    });
    expect(await answer).toEqual(OK);
    expect(client.lastRequestId()).toBe('req-1a2b');
    expect(client.metaVersion()).toBe('m-changed');
    expect(client.replayed()).toBe(false);
    expect(metaChanges).toEqual(['m-changed']);
    const refetch = http.expectOne({ method: 'GET', url: '/api/v1/boot' });
    expect(refetch.request.headers.get('If-None-Match')).toBe(`"${BOOT.meta}"`);
    refetch.flush({ ...BOOT, meta: 'm-changed' }, { headers: { ETag: '"m-changed"' } });
    await new Promise((done) => setTimeout(done, 0));
    expect(TestBed.inject(KeelBoot).meta()).toBe('m-changed');

    const again = call(client, 'dns.zone.edit', params);
    http.expectOne('/api/v1/call/dns.zone.edit').flush(OK, {
      headers: { 'X-Keel-Request-Id': 'req-1a2c', 'X-Keel-Meta-Version': 'm-changed' },
    });
    await again;
    expect(client.lastRequestId()).toBe('req-1a2c');
    expect(metaChanges).toEqual(['m-changed']);
  });

  it('a bearer option sets the header and nothing else does', async () => {
    const { client, http } = harness();

    const withBearer = call(
      client,
      'dns.zone.list',
      {},
      {
        bearer: 'kat_01J9',
        idempotencyKey: 'bill-order-9912-open',
        requestId: 'trace_7.a',
      },
    );
    const first = http.expectOne('/api/v1/call/dns.zone.list');
    expect(first.request.headers.get('Authorization')).toBe('Bearer kat_01J9');
    expect(first.request.headers.get('Idempotency-Key')).toBe('bill-order-9912-open');
    expect(first.request.headers.get('X-Keel-Request-Id')).toBe('trace_7.a');
    first.flush(OK);
    await withBearer;

    const without = call(client, 'dns.zone.list', {});
    const second = http.expectOne('/api/v1/call/dns.zone.list');
    expect(second.request.headers.keys()).toEqual([]);
    expect(second.request.headers.has('Authorization')).toBe(false);
    expect(second.request.headers.has('Idempotency-Key')).toBe(false);
    expect(second.request.headers.has('X-Keel-Request-Id')).toBe(false);
    second.flush(OK);
    await without;
  });

  it('a 401 keel.error.unauthenticated emits unauthenticated once, while a 401 of a function and a 403 cross_site are returned as the envelope', async () => {
    const { client, http, boot } = harness();
    const state = { bursts: 0 };
    client.unauthenticated.subscribe(() => {
      state.bursts += 1;
    });

    const first = call(client, 'dns.zone.list', {});
    const second = call(client, 'dns.zone.get', { id: 'x' });
    for (const request of http.match(() => true)) {
      request.flush(UNAUTHENTICATED, { status: 401, statusText: 'Unauthorized' });
    }
    expect(await first).toEqual(UNAUTHENTICATED);
    expect(await second).toEqual(UNAUTHENTICATED);
    expect(state.bursts).toBe(1);
    expect(client.lastRequestId()).toBe('req-401');

    const refused = call(client, 'dns.zone.edit', { id: 'x' });
    http
      .expectOne('/api/v1/call/dns.zone.edit')
      .flush(CROSS_SITE, { status: 403, statusText: 'Forbidden' });
    const envelope = await refused;
    expect(envelope.ok).toBe(false);
    expect(envelope).toEqual(CROSS_SITE);
    expect(state.bursts).toBe(1);

    await boot();
    const wrongCode = call(client, 'auth.totp.confirm', { code: '000000' });
    http
      .expectOne('/api/v1/call/auth.totp.confirm')
      .flush(WRONG_CODE, { status: 401, statusText: 'Unauthorized' });
    expect(await wrongCode).toEqual(WRONG_CODE);
    expect(state.bursts).toBe(1);
    expect(client.lastRequestId()).toBe('req-code');

    const later = call(client, 'dns.zone.list', {});
    http
      .expectOne('/api/v1/call/dns.zone.list')
      .flush(UNAUTHENTICATED, { status: 401, statusText: 'Unauthorized' });
    await later;
    expect(state.bursts).toBe(2);
  });

  it('a write with an idempotency key is retried once on a network failure and a call without one never', async () => {
    const { client, http } = harness();

    const keyed = call(client, 'dns.zone.edit', { id: 'x' }, { idempotencyKey: 'k-1' });
    http.expectOne('/api/v1/call/dns.zone.edit').error(new ProgressEvent('error'));
    const retried = http.expectOne('/api/v1/call/dns.zone.edit');
    expect(retried.request.headers.get('Idempotency-Key')).toBe('k-1');
    expect(retried.request.body).toEqual({ id: 'x' });
    retried.flush(OK, { headers: { 'X-Keel-Idempotent-Replay': 'true' } });
    expect(await keyed).toEqual(OK);
    expect(client.replayed()).toBe(true);

    const twice = call(client, 'dns.zone.edit', { id: 'x' }, { idempotencyKey: 'k-2' });
    http.expectOne('/api/v1/call/dns.zone.edit').error(new ProgressEvent('error'));
    http.expectOne('/api/v1/call/dns.zone.edit').error(new ProgressEvent('error'));
    await expect(twice).rejects.toMatchObject({ status: 0 });

    const bare = call(client, 'dns.zone.edit', { id: 'x' });
    http.expectOne('/api/v1/call/dns.zone.edit').error(new ProgressEvent('error'));
    await expect(bare).rejects.toMatchObject({ status: 0 });
    http.expectNone('/api/v1/call/dns.zone.edit');
  });

  it('an in-flight call is counted into the global progress', async () => {
    const { client, http, progress } = harness();
    expect(progress.active()).toBe(false);
    const pending = call(client, 'dns.zone.list', {});
    expect(progress.active()).toBe(true);
    http.expectOne('/api/v1/call/dns.zone.list').flush(OK);
    await pending;
    expect(progress.active()).toBe(false);
  });

  it('an answer that is not an envelope is an error rather than a result', async () => {
    const { client, http } = harness();
    const pending = call(client, 'dns.zone.list', {});
    http.expectOne('/api/v1/call/dns.zone.list').flush('<html>');
    await expect(pending).rejects.toThrow(
      'dns.zone.list answered something other than a call envelope',
    );
  });
});
