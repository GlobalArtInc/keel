import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { KeelAuth, KeelBoot, KeelDescriptors, provideKeelClient } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, MetaPrincipal } from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;
const LIST = JSON.parse(
  readFileSync(join(FIXTURES, 'descriptor-dns-zone-list.json'), 'utf8'),
) as Descriptor;

const PRINCIPAL: MetaPrincipal = {
  id: '01J9Q3W6B1R8N5X2K4T7M0V9A1',
  login: 'alice',
  level: 'user',
  account: '01J8A0000000000000000000A1',
  actor: null,
};

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function ok<T>(data: T): Envelope<T> {
  return { ok: true, data, meta: { requestId: 'req-auth', principal: PRINCIPAL } };
}

describe('KeelAuth', () => {
  let auth: KeelAuth;
  let boot: KeelBoot;
  let http: HttpTestingController;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
    });
    auth = TestBed.inject(KeelAuth);
    boot = TestBed.inject(KeelBoot);
    http = TestBed.inject(HttpTestingController);
  });

  afterEach(() => http.verify());

  async function answer<T>(func: string, body: unknown, data: T, pending: Promise<Envelope<T>>) {
    const request = http.expectOne({ method: 'POST', url: `/api/v1/call/${func}` });
    expect(request.request.body).toEqual(body);
    request.flush(ok(data));
    await settled();
    http.expectOne({ method: 'GET', url: '/api/v1/boot' }).flush(BOOT);
    const envelope = await pending;
    expect(envelope).toEqual(ok(data));
    http.expectNone('/api/v1/boot');
    return envelope;
  }

  it('login posts the credentials and answers the session or the totp challenge', async () => {
    await answer(
      'auth.login',
      { login: 'alice', password: 'pa55' },
      { state: 'authenticated', principal: PRINCIPAL },
      auth.login({ login: 'alice', password: 'pa55' }),
    );
    expect(boot.principal()?.login).toBe('res1');

    const challenged = await answer(
      'auth.login',
      { login: 'alice', password: 'pa55', method: 'local' },
      { state: 'totp', challenge: 'ch_01J9' },
      auth.login({ login: 'alice', password: 'pa55', method: 'local' }),
    );
    if (challenged.ok && challenged.data.state === 'totp') {
      expect(challenged.data.challenge).toBe('ch_01J9');
    } else {
      throw new Error('the totp answer was not recognised');
    }
  });

  it('verifyTotp posts the challenge and the code', async () => {
    await answer(
      'auth.totp.verify',
      { challenge: 'ch_01J9', code: '123456' },
      { state: 'authenticated', principal: PRINCIPAL },
      auth.verifyTotp({ challenge: 'ch_01J9', code: '123456' }),
    );
  });

  it('redeemKey posts the key and answers the redirect', async () => {
    const redeemed = await answer(
      'auth.key.redeem',
      { key: 'kok_01J9' },
      {
        redirect: '/f/dns.zone.list',
        principal: PRINCIPAL,
        issuedBy: '01J8A0000000000000000000B2',
      },
      auth.redeemKey({ key: 'kok_01J9' }),
    );
    expect(redeemed.ok && redeemed.data.redirect).toBe('/f/dns.zone.list');
  });

  it('logout, su and suExit post to their names and drop the descriptors of the principal', async () => {
    const descriptors = TestBed.inject(KeelDescriptors);
    const cached = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    expect(await cached).toEqual(LIST);

    await answer('auth.logout', {}, { revoked: true }, auth.logout());
    const again = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    expect(await again).toEqual(LIST);

    await answer(
      'auth.su',
      { principal: PRINCIPAL.id },
      { principal: PRINCIPAL },
      auth.su({ principal: PRINCIPAL.id }),
    );
    const delegated = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    expect(await delegated).toEqual(LIST);

    await answer('auth.su.exit', {}, { principal: PRINCIPAL }, auth.suExit());
    const returned = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    expect(await returned).toEqual(LIST);
  });

  it('setupAdmin posts the key, the login and the password and reloads nothing', async () => {
    const pending = auth.setupAdmin({
      key: 'kok_01J9',
      login: 'first',
      password: 'hunter2hunter2',
    });
    const request = http.expectOne({ method: 'POST', url: '/api/v1/call/keel.setup.admin' });
    expect(request.request.body).toEqual({
      key: 'kok_01J9',
      login: 'first',
      password: 'hunter2hunter2',
    });
    const created = ok({ id: PRINCIPAL.id, login: 'first' });
    request.flush(created);
    expect(await pending).toEqual(created);
    http.expectNone('/api/v1/boot');
  });

  it('a refused login is answered as its envelope without a boot reload', async () => {
    const pending = auth.login({ login: 'alice', password: 'wrong' });
    const refused: Envelope = {
      ok: false,
      error: { code: 'unauthenticated', key: 'auth.error.credentials', requestId: 'req-no' },
    };
    http
      .expectOne('/api/v1/call/auth.login')
      .flush(refused, { status: 401, statusText: 'Unauthorized' });
    expect(await pending).toEqual(refused);
    await settled();
    http.expectNone('/api/v1/boot');
    expect(boot.loaded()).toBe(false);
  });
});
