import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { KeelBoot, KeelCallError, KeelClient, provideKeelClient } from '@globalart/keel/client';
import type { Boot } from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;

describe('KeelBoot', () => {
  let boot: KeelBoot;
  let http: HttpTestingController;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
    });
    boot = TestBed.inject(KeelBoot);
    http = TestBed.inject(HttpTestingController);
  });

  afterEach(() => http.verify());

  it('load keeps the payload as signals and a refetch sends If-None-Match', async () => {
    expect(boot.loaded()).toBe(false);
    expect(boot.menu()).toEqual([]);

    const first = boot.load();
    const request = http.expectOne({ method: 'GET', url: '/api/v1/boot' });
    expect(request.request.headers.has('If-None-Match')).toBe(false);
    request.flush(BOOT, { headers: { ETag: `"${BOOT.meta}"` } });
    expect(await first).toEqual(BOOT);
    expect(boot.loaded()).toBe(true);
    expect(boot.principal()?.login).toBe('res1');
    expect(boot.products().map((product) => product.id)).toEqual(['dns', 'panel']);
    expect(boot.brand()?.name).toBe('keel.brand');
    expect(boot.menu().map((group) => group.id)).toEqual(['dns', 'hosting', 'system']);
    expect(boot.views()).toEqual([]);
    expect(boot.features()).toEqual([]);
    expect(boot.meta()).toBe(BOOT.meta);
    expect(TestBed.inject(KeelClient).metaVersion()).toBe(BOOT.meta);

    const second = boot.load();
    const refetch = http.expectOne('/api/v1/boot');
    expect(refetch.request.headers.get('If-None-Match')).toBe(`"${BOOT.meta}"`);
    refetch.flush(null, { status: 304, statusText: 'Not Modified' });
    expect(await second).toEqual(BOOT);
    expect(boot.principal()?.login).toBe('res1');

    const third = boot.load();
    const changed = { ...BOOT, meta: 'm-next', views: ['hosting.file-manager'] };
    http.expectOne('/api/v1/boot').flush(changed, { headers: { ETag: '"m-next"' } });
    expect(await third).toEqual(changed);
    expect(boot.views()).toEqual(['hosting.file-manager']);
    expect(TestBed.inject(KeelClient).metaVersion()).toBe('m-next');

    boot.drop();
    expect(boot.loaded()).toBe(false);
    const fourth = boot.load();
    const fresh = http.expectOne('/api/v1/boot');
    expect(fresh.request.headers.has('If-None-Match')).toBe(false);
    fresh.flush(BOOT);
    await fourth;
  });

  it('a 401 on boot emits unauthenticated and rejects the load', async () => {
    const bursts: number[] = [];
    TestBed.inject(KeelClient).unauthenticated.subscribe(() => bursts.push(1));
    const pending = boot.load();
    http
      .expectOne('/api/v1/boot')
      .flush(
        { ok: false, error: { code: 'unauthenticated', key: 'keel.error.unauthenticated' } },
        { status: 401, statusText: 'Unauthorized' },
      );
    await expect(pending).rejects.toBeInstanceOf(KeelCallError);
    await expect(pending).rejects.toMatchObject({ error: { code: 'unauthenticated' } });
    expect(bursts).toHaveLength(1);
    expect(boot.loaded()).toBe(false);
  });
});
