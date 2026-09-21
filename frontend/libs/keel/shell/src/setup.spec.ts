import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { Component } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KEEL_EVENT_SOURCE, KeelClient } from '@globalart/keel/client';
import type { Boot, Envelope, MetaPrincipal } from '@globalart/keel/meta';
import { provideKeelViews } from '@globalart/keel/renderer';
import { keelProviders, type KeelProduct } from '@globalart/keel/shell';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const TAG_CLOUD = 'note.tag.list';
const SCREEN = `/f/${TAG_CLOUD}`;
const KEY = 'kok_0123';
const SETUP_ADMIN = 'keel.setup.admin';

const PRINCIPAL: MetaPrincipal = {
  id: '01J9Q3W6B1R8N5X2K4T7M0V9A1',
  login: 'first',
  level: 'admin',
  account: '00000000000000000000000000',
  actor: null,
};

const MESSAGES = {
  keel: {
    brand: 'Keel',
    error: { function_not_found: 'The function {function} does not exist' },
    setup: { error: { key: 'The setup link is not valid' } },
    shell: {
      setup: {
        title: 'Create the first administrator',
        intro: 'This installation has no administrator yet.',
        login: 'Login',
        password: 'Password',
        submit: 'Create the administrator',
        failed: 'The administrator was not created',
        no_key: 'The address carries no setup key',
        done: 'This installation already has an administrator',
        signing_in: 'Signing in as the new administrator',
        to_login: 'Go to the sign-in page',
      },
    },
  },
  auth: {
    error: { password_short: 'At least {min} characters' },
  },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function demoBoot(): Boot {
  const boot = load<Boot>('boot-none.json');
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
  return { ok: true, data, meta: { requestId: 'req-setup', principal: PRINCIPAL } };
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
    providers: [
      ...keelProviders({ products: [DEMO] }),
      { provide: KeelClient, useValue: client },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
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

function submit(root: Element): void {
  const form = root.querySelector<HTMLFormElement>('form[data-step="setup"]');
  if (form === null) {
    throw new Error('the setup form is not shown');
  }
  form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
}

function fill(root: Element, login: string, password: string): void {
  type(root, 'form[data-step="setup"] phx-input input', login);
  type(root, 'form[data-step="setup"] phx-password input', password);
  submit(root);
}

function banner(root: Element): { title: string | null; text: string | null } {
  const shown = root.querySelector('phx-banner');
  return {
    title: shown?.querySelector('.title')?.textContent?.trim() ?? null,
    text: shown?.querySelector('.text')?.textContent?.trim() ?? null,
  };
}

describe('the setup page', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('without a key it shows why and offers the sign-in page instead of the form', async () => {
    const client = configure({});
    const harness = await RouterTestingHarness.create('/setup');
    const element: HTMLElement = harness.fixture.nativeElement;
    const setup = element.querySelector('keel-setup');
    expect(setup?.getAttribute('data-stage')).toBe('form');
    expect(element.querySelector('form[data-step="setup"]')).toBeNull();
    expect(element.querySelector('phx-logon .title')?.textContent).toBe(
      'Create the first administrator',
    );
    expect(banner(element).text).toBe('The address carries no setup key');
    expect(element.querySelector('a.to-login')?.getAttribute('href')).toBe('/login');
    expect(client.calls).toHaveLength(0);
  });

  it('the form posts keel.setup.admin with the key, shows a refused password from its field key, then signs the administrator in and lands home', async () => {
    const client = configure({
      [SETUP_ADMIN]: [
        {
          ok: false,
          error: {
            code: 'validation',
            key: 'keel.error.validation',
            requestId: 'req-short',
            fields: [{ path: 'password', key: 'auth.error.password_short', params: { min: 12 } }],
          },
        },
        ok({ id: PRINCIPAL.id, login: 'first' }),
      ],
      'auth.login': ok({ state: 'authenticated', principal: PRINCIPAL }),
    });
    const harness = await RouterTestingHarness.create(`/setup?key=${KEY}`);
    const router = TestBed.inject(Router);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('.intro')?.textContent?.trim()).toBe(
      'This installation has no administrator yet.',
    );

    submit(element);
    await settle(harness.fixture);
    expect(client.calls).toHaveLength(0);

    fill(element, 'first', 'short');
    await settle(harness.fixture);
    expect(client.callsOf(SETUP_ADMIN).map((call) => call.params)).toEqual([
      { key: KEY, login: 'first', password: 'short' },
    ]);
    expect(banner(element)).toEqual({
      title: 'The administrator was not created',
      text: 'At least 12 characters',
    });
    expect(element.querySelector('keel-setup')?.getAttribute('data-stage')).toBe('form');

    type(element, 'form[data-step="setup"] phx-password input', 'correct-horse-battery');
    submit(element);
    await settle(harness.fixture);
    expect(client.callsOf(SETUP_ADMIN)).toHaveLength(2);
    expect(client.callsOf('auth.login').map((call) => call.params)).toEqual([
      { login: 'first', password: 'correct-horse-battery' },
    ]);
    expect(router.url).toBe(SCREEN);
    expect(element.querySelector('keel-layout')).not.toBeNull();
    expect(element.querySelector('keel-setup')).toBeNull();
  });

  it('a not_found answer means the installation has its administrator and says so', async () => {
    const client = configure({
      [SETUP_ADMIN]: {
        ok: false,
        error: {
          code: 'not_found',
          key: 'keel.error.function_not_found',
          params: { function: SETUP_ADMIN },
          requestId: 'req-gone',
        },
      },
    });
    const harness = await RouterTestingHarness.create(`/setup?key=${KEY}`);
    const element: HTMLElement = harness.fixture.nativeElement;
    fill(element, 'second', 'correct-horse-battery');
    await settle(harness.fixture);
    expect(client.callsOf(SETUP_ADMIN)).toHaveLength(1);
    expect(client.callsOf('auth.login')).toHaveLength(0);
    expect(element.querySelector('keel-setup')?.getAttribute('data-stage')).toBe('done');
    expect(element.querySelector('form[data-step="setup"]')).toBeNull();
    expect(banner(element).text).toBe('This installation already has an administrator');
    expect(TestBed.inject(Router).url).toBe(`/setup?key=${KEY}`);
  });
});
