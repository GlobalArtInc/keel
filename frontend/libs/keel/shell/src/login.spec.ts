import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { Component } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AUTH_LOGOUT, KeelBoot, KeelClient } from '@globalart/keel/client';
import type { Boot, Envelope, ErrorCode, MetaPrincipal } from '@globalart/keel/meta';
import { provideKeelViews } from '@globalart/keel/renderer';
import { KeelShellComponent, keelProviders, type KeelProduct } from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const TAG_CLOUD = 'note.tag.list';
const SCREEN = `/f/${TAG_CLOUD}`;
const ENCODED_SCREEN = encodeURIComponent(SCREEN);

const PRINCIPAL: MetaPrincipal = {
  id: '01J9Q3W6B1R8N5X2K4T7M0V9A1',
  login: 'alice',
  level: 'user',
  account: '01J8A0000000000000000000A1',
  actor: null,
};

const MESSAGES = {
  keel: {
    brand: 'Keel',
    action: { back: 'Back' },
    error: { unauthenticated: 'Sign in to continue' },
    shell: {
      login: {
        title: 'Sign in',
        login: 'Login',
        password: 'Password',
        submit: 'Sign in',
        code: 'Code',
        code_hint: 'Enter the code from your authenticator app, or a recovery code',
        verify: 'Verify',
        key: 'Signing in with the login key',
        key_failed: 'The login key was not accepted',
        failed: 'Sign-in failed',
      },
    },
  },
  auth: {
    error: {
      credentials: 'The login or the password is wrong',
      locked: 'Signing in is locked for this login from your address; try again later',
      code: 'The code is wrong',
      key: 'The login key is not valid; it may have been used or expired',
    },
  },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function demoBoot(): Boot {
  const boot = load<Boot>('boot-none.json');
  boot.products = [
    {
      id: 'demo',
      name: 'product.demo.brand',
      edition: null,
      brand: { name: 'product.demo.brand', hue: 150, chroma: 0.1 },
    },
  ];
  boot.menu.unshift({
    id: 'demo',
    product: 'demo',
    label: 'menu.demo',
    icon: 'tag',
    items: [{ path: 'demo/tags', label: 'note.tag.list.title', func: TAG_CLOUD, icon: 'tag' }],
  });
  boot.views = [TAG_CLOUD];
  return boot;
}

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-login', principal: PRINCIPAL } };
}

function refused(key: string, code: ErrorCode = 'unauthenticated'): Envelope {
  return { ok: false, error: { code, key, requestId: 'req-refused' } };
}

@Component({ selector: 'keel-test-tag-cloud', template: '<p class="cloud"></p>' })
class TagCloudComponent {}

const DEMO: KeelProduct = {
  id: 'demo',
  load: async () => provideKeelViews([{ func: TAG_CLOUD, component: TagCloudComponent }]),
};

function configure(answers: Record<string, Envelope | Envelope[]>): KeelMockClient {
  const client = new KeelMockClient({
    boot: demoBoot(),
    catalogue: { lang: 'en', messages: MESSAGES },
    answers,
  });
  TestBed.configureTestingModule({
    providers: [...keelProviders({ products: [DEMO] }), { provide: KeelClient, useValue: client }],
  });
  return client;
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

function type(root: Element, selector: string, value: string): void {
  const input = root.querySelector<HTMLInputElement>(selector);
  if (input === null) {
    throw new Error(`no input matches ${selector}`);
  }
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

function submit(root: Element, step: string): void {
  const form = root.querySelector<HTMLFormElement>(`form[data-step="${step}"]`);
  if (form === null) {
    throw new Error(`the ${step} step is not shown`);
  }
  form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
}

function banner(root: Element): { title: string | null; text: string | null } {
  const shown = root.querySelector('phx-banner');
  return {
    title: shown?.querySelector('.title')?.textContent?.trim() ?? null,
    text: shown?.querySelector('.text')?.textContent?.trim() ?? null,
  };
}

describe('the login page', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('the password step posts auth.login, a totp answer opens the code step, which posts auth.totp.verify with the challenge, and returnTo is honoured', async () => {
    const client = configure({
      'auth.login': [
        refused('auth.error.credentials'),
        ok({ state: 'totp', challenge: 'ch_01J9' }),
      ],
      'auth.totp.verify': [
        refused('auth.error.code'),
        ok({ state: 'authenticated', principal: PRINCIPAL }),
      ],
    });
    const harness = await RouterTestingHarness.create(`/login?returnTo=${ENCODED_SCREEN}`);
    const router = TestBed.inject(Router);
    const element: HTMLElement = harness.fixture.nativeElement;
    const login = element.querySelector('keel-login');
    expect(login?.getAttribute('data-step')).toBe('password');
    expect(element.querySelector('phx-logon .title')?.textContent).toBe('Sign in');
    expect(element.querySelector('.brand')?.textContent).toBe('Keel');

    submit(element, 'password');
    await settle(harness.fixture);
    expect(client.calls).toHaveLength(0);

    type(element, 'form[data-step="password"] phx-input input', 'alice');
    type(element, 'form[data-step="password"] phx-password input', 'wrong');
    submit(element, 'password');
    await settle(harness.fixture);
    expect(client.callsOf('auth.login').map((call) => call.params)).toEqual([
      { login: 'alice', password: 'wrong' },
    ]);
    expect(login?.getAttribute('data-step')).toBe('password');
    expect(banner(element)).toEqual({ title: null, text: 'The login or the password is wrong' });
    expect(router.url).toBe(`/login?returnTo=${ENCODED_SCREEN}`);

    type(element, 'form[data-step="password"] phx-password input', 'pa55');
    submit(element, 'password');
    await settle(harness.fixture);
    expect(client.callsOf('auth.login')).toHaveLength(2);
    expect(login?.getAttribute('data-step')).toBe('code');
    expect(element.querySelector('phx-banner')).toBeNull();
    expect(element.querySelector('form[data-step="code"] .hint')?.textContent?.trim()).toBe(
      'Enter the code from your authenticator app, or a recovery code',
    );
    expect(element.querySelector('.back')?.textContent?.trim()).toBe('Back');

    type(element, 'form[data-step="code"] phx-input input', '000000');
    submit(element, 'code');
    await settle(harness.fixture);
    expect(client.callsOf('auth.totp.verify').map((call) => call.params)).toEqual([
      { challenge: 'ch_01J9', code: '000000' },
    ]);
    expect(banner(element).text).toBe('The code is wrong');

    type(element, 'form[data-step="code"] phx-input input', 'rc-recovery-1');
    submit(element, 'code');
    await settle(harness.fixture);
    expect(client.callsOf('auth.totp.verify')[1].params).toEqual({
      challenge: 'ch_01J9',
      code: 'rc-recovery-1',
    });
    expect(router.url).toBe(SCREEN);
    expect(element.querySelector('keel-test-tag-cloud')).not.toBeNull();
    expect(TestBed.inject(KeelBoot).principal()?.login).toBe('owner');
  });

  it('the back link of the code step returns to the password step', async () => {
    configure({ 'auth.login': ok({ state: 'totp', challenge: 'ch_01J9' }) });
    const harness = await RouterTestingHarness.create('/login');
    const element: HTMLElement = harness.fixture.nativeElement;
    type(element, 'form[data-step="password"] phx-input input', 'alice');
    type(element, 'form[data-step="password"] phx-password input', 'pa55');
    submit(element, 'password');
    await settle(harness.fixture);
    expect(element.querySelector('keel-login')?.getAttribute('data-step')).toBe('code');
    element.querySelector<HTMLButtonElement>('.back')?.click();
    await settle(harness.fixture);
    expect(element.querySelector('keel-login')?.getAttribute('data-step')).toBe('password');
    expect(element.querySelector<HTMLInputElement>('phx-input input')?.value).toBe('alice');
    expect(element.querySelector<HTMLInputElement>('phx-password input')?.value).toBe('');
  });

  it('a lockout is shown from its key and returnTo outside the installation falls back to /', async () => {
    configure({
      'auth.login': [
        refused('auth.error.locked', 'forbidden'),
        ok({ state: 'authenticated', principal: PRINCIPAL }),
      ],
    });
    const harness = await RouterTestingHarness.create(
      `/login?returnTo=${encodeURIComponent('https://elsewhere.example/f/x')}`,
    );
    const router = TestBed.inject(Router);
    const element: HTMLElement = harness.fixture.nativeElement;
    type(element, 'phx-input input', 'alice');
    type(element, 'phx-password input', 'pa55');
    submit(element, 'password');
    await settle(harness.fixture);
    expect(banner(element).text).toBe(
      'Signing in is locked for this login from your address; try again later',
    );
    submit(element, 'password');
    await settle(harness.fixture);
    expect(router.url).toBe(SCREEN);
  });

  it('/login?key= posts auth.key.redeem on load and navigates to redirect', async () => {
    const client = configure({
      'auth.key.redeem': ok({ redirect: SCREEN, principal: PRINCIPAL, issuedBy: PRINCIPAL.id }),
    });
    const harness = await RouterTestingHarness.create('/login?key=kok_01J9');
    await settle(harness.fixture);
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['auth.key.redeem', { key: 'kok_01J9' }],
    ]);
    expect(TestBed.inject(Router).url).toBe(SCREEN);
    expect(harness.fixture.nativeElement.querySelector('keel-test-tag-cloud')).not.toBeNull();
  });

  it('a refused key shows why, drops the key from the address and offers the password step', async () => {
    const client = configure({ 'auth.key.redeem': refused('auth.error.key') });
    const harness = await RouterTestingHarness.create('/login?key=kok_used&returnTo=%2Fx');
    await settle(harness.fixture);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(client.calls).toHaveLength(1);
    expect(TestBed.inject(Router).url).toBe('/login?returnTo=%2Fx');
    expect(element.querySelector('keel-login')?.getAttribute('data-step')).toBe('password');
    expect(banner(element)).toEqual({
      title: 'The login key was not accepted',
      text: 'The login key is not valid; it may have been used or expired',
    });
  });

  it('a 401 from any call lands on /login with returnTo set to the screen it came from, except on the login page itself', async () => {
    const client = configure({});
    const fixture = TestBed.createComponent(KeelShellComponent);
    const router = TestBed.inject(Router);
    const boot = TestBed.inject(KeelBoot);
    fixture.detectChanges();
    expect(fixture.nativeElement.getAttribute('data-startup')).toBe('loading');
    await router.navigateByUrl(`${SCREEN}?tab=all`);
    await settle(fixture);
    expect(fixture.nativeElement.getAttribute('data-startup')).toBe('ready');
    expect(fixture.nativeElement.querySelector('keel-test-tag-cloud')).not.toBeNull();
    const loads = jest.spyOn(client, 'boot');

    client.expire();
    await settle(fixture);
    expect(router.url).toBe(`/login?returnTo=${encodeURIComponent(`${SCREEN}?tab=all`)}`);
    expect(fixture.nativeElement.querySelector('keel-login')).not.toBeNull();
    expect(fixture.nativeElement.querySelector('keel-layout')).toBeNull();
    expect(loads).toHaveBeenCalledTimes(1);
    expect(boot.loaded()).toBe(true);

    client.expire();
    await settle(fixture);
    expect(router.url).toBe(`/login?returnTo=${encodeURIComponent(`${SCREEN}?tab=all`)}`);
    expect(loads).toHaveBeenCalledTimes(1);
  });

  it('a 401 that arrives while the logout is already on its way leaves the plain /login in place', async () => {
    const client = configure({ [AUTH_LOGOUT]: ok({}) });
    const fixture = TestBed.createComponent(KeelShellComponent);
    const router = TestBed.inject(Router);
    fixture.detectChanges();
    await router.navigateByUrl(SCREEN);
    await settle(fixture);
    expect(fixture.nativeElement.querySelector('keel-test-tag-cloud')).not.toBeNull();

    const leaving = router.navigateByUrl('/logout');
    client.expire();
    await expect(leaving).resolves.toBe(true);
    await settle(fixture);
    expect(client.callsOf(AUTH_LOGOUT)).toHaveLength(1);
    expect(router.url).toBe('/login');
    expect(fixture.nativeElement.querySelector('keel-login')).not.toBeNull();
  });
});
