import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { ApplicationRef } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Action, Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import {
  KeelActionRunner,
  KeelScreenComponent,
  bodyOf,
  routeAction,
  substitute,
  type ActionOutcome,
  type Row,
} from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const PRODUCTS = load<Descriptor>('descriptors/keel.product.list.json');
const ENABLE = load<Descriptor>('descriptors/keel.product.enable.json');
const BOOT = load<Boot>('boot.json');
const ZONES: Row[] = [
  { id: 'z1', name: 'a.org', records: 3, status: 'active' },
  { id: 'z2', name: 'b.org', records: 5, status: 'active' },
];

const MESSAGES = {
  keel: {
    action: { delete: 'Delete', retry: 'Retry', done: '{action} completed' },
    confirm: {
      one: 'Continue with {action}?',
      many: 'Continue with {action} for {count} items?',
      count: 'Type {count} to confirm',
    },
    error: {
      forbidden: 'You are not allowed to do this',
      request: '{message} (request {id})',
      unavailable: 'Temporarily unavailable',
    },
    product: {
      action: { enable: 'Enable', disable: 'Disable' },
      confirm: { enable: 'Enable the product {id}? The service restarts to load its modules.' },
      state: { enabled: 'Enabled' },
      column: { id: 'Product', state: 'State' },
    },
  },
  dns: { zone: { action: { suspend: 'Suspend' } } },
};

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

function action(id: string): Action {
  const found = LIST.ui?.actions?.find((entry) => entry.id === id);
  if (found === undefined) {
    throw new Error(`no action ${id}`);
  }
  return found;
}

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-ok' } };
}

async function runner(client: KeelMockClient): Promise<KeelActionRunner> {
  TestBed.configureTestingModule({
    providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  return TestBed.inject(KeelActionRunner);
}

function tick(): void {
  TestBed.inject(ApplicationRef).tick();
}

function dialogs(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('.cdk-overlay-container phx-confirm')];
}

function dialog(): HTMLElement {
  const open = dialogs();
  if (open.length !== 1) {
    throw new Error(`${open.length} confirm dialogs are open`);
  }
  return open[0];
}

function confirmButton(): HTMLButtonElement {
  const button = dialog().querySelector<HTMLButtonElement>('.buttons button');
  if (button === null) {
    throw new Error('no confirm button');
  }
  return button;
}

function cancelButton(): HTMLButtonElement {
  const button = dialog().querySelectorAll<HTMLButtonElement>('.buttons button')[1];
  if (button === undefined) {
    throw new Error('no cancel button');
  }
  return button;
}

function type(value: string): void {
  const input = dialog().querySelector<HTMLInputElement>('phx-input input');
  if (input === null) {
    throw new Error('no confirm input');
  }
  input.value = value;
  input.dispatchEvent(new Event('input'));
  tick();
}

async function settle(): Promise<void> {
  await new Promise((done) => setTimeout(done, 0));
}

describe('the action runner', () => {
  afterEach(() => {
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('a destructive confirm lists the items and requires the count', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'dns.zone.delete': ok({ deleted: 2 }) },
    });
    const run = (await runner(client)).run({
      descriptor: LIST,
      action: action('delete'),
      rows: ZONES,
    });
    tick();
    const open = dialog();
    expect(open.querySelector('.title')?.textContent).toBe('Delete');
    expect(open.querySelector('.text')?.textContent).toBe('Continue with Delete for 2 items?');
    expect([...open.querySelectorAll('.item')].map((item) => item.textContent)).toEqual([
      'a.org',
      'b.org',
    ]);
    expect(open.querySelector('phx-input .frame__label')?.textContent?.trim()).toBe(
      'Type 2 to confirm',
    );
    expect(confirmButton().className).toContain('danger');
    expect(confirmButton().disabled).toBe(true);
    expect(client.calls).toHaveLength(0);

    type('3');
    expect(confirmButton().disabled).toBe(true);
    confirmButton().click();
    await settle();
    expect(client.calls).toHaveLength(0);
    expect(dialogs()).toHaveLength(1);

    type('2');
    expect(confirmButton().disabled).toBe(false);
    confirmButton().click();
    const outcome = await run;
    expect(dialogs()).toHaveLength(0);
    expect(outcome?.ok).toBe(true);
    expect(client.calls).toHaveLength(1);
    expect(client.calls[0].func).toBe('dns.zone.delete');
    expect(client.calls[0].params).toEqual({ ids: ['z1', 'z2'] });
    expect(client.calls[0].options.idempotencyKey).toMatch(UUID);
    expect(TestBed.inject(PhxToastService).toasts()).toHaveLength(0);
  });

  it('confirm: true asks once and a refusal posts nothing', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'dns.zone.suspend': ok({}) },
    });
    const run = (await runner(client)).run({
      descriptor: LIST,
      action: action('suspend'),
      rows: [ZONES[0]],
    });
    tick();
    const open = dialog();
    expect(open.querySelector('.title')?.textContent).toBe('Suspend');
    expect(open.querySelector('.text')?.textContent).toBe('Continue with Suspend?');
    expect(open.querySelector('.items')).toBeNull();
    expect(open.querySelector('phx-input')).toBeNull();
    expect(confirmButton().disabled).toBe(false);
    cancelButton().click();
    const outcome: ActionOutcome = await run;
    expect(outcome).toBeNull();
    expect(dialogs()).toHaveLength(0);
    expect(client.calls).toHaveLength(0);

    const again = TestBed.inject(KeelActionRunner).run({
      descriptor: LIST,
      action: action('suspend'),
      rows: [ZONES[0]],
    });
    tick();
    expect(dialogs()).toHaveLength(1);
    confirmButton().click();
    expect((await again)?.ok).toBe(true);
    expect(dialogs()).toHaveLength(0);
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['dns.zone.suspend', { ids: ['z1'] }],
    ]);
  });

  it('a confirmation text of the action itself replaces the generic question', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'keel.product.enable': ok({ id: 'dns', state: 'enabled', changed: true, restart: true }),
        'keel.product.disable': ok({ id: 'dns', state: 'disabled', changed: true, restart: true }),
      },
    });
    const actions = PRODUCTS.ui?.actions ?? [];
    const enable = actions.find((entry) => entry.id === 'enable');
    const disable = actions.find((entry) => entry.id === 'disable');
    if (enable === undefined || disable === undefined) {
      throw new Error('the products screen names its two actions');
    }
    const row: Row = { id: 'dns', state: 'disabled' };
    const run = (await runner(client)).run({ descriptor: PRODUCTS, action: enable, rows: [row] });
    tick();
    const open = dialog();
    expect(open.querySelector('.title')?.textContent).toBe('Enable');
    expect(open.querySelector('.text')?.textContent).toBe(
      'Enable the product dns? The service restarts to load its modules.',
    );
    confirmButton().click();
    expect((await run)?.ok).toBe(true);
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['keel.product.enable', { id: 'dns' }],
    ]);

    const fallback = TestBed.inject(KeelActionRunner).run({
      descriptor: PRODUCTS,
      action: disable,
      rows: [row],
    });
    tick();
    expect(dialog().querySelector('.text')?.textContent).toBe('Continue with Disable?');
    cancelButton().click();
    expect(await fallback).toBeNull();
  });

  it('a failure shows a toast with the message and the request id', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'dns.zone.delete': {
          ok: false,
          error: { code: 'forbidden', key: 'keel.error.forbidden', requestId: 'req-9' },
        },
      },
    });
    const run = (await runner(client)).run({
      descriptor: LIST,
      action: action('delete'),
      rows: [ZONES[1]],
    });
    tick();
    type('1');
    confirmButton().click();
    const outcome = await run;
    expect(outcome?.ok).toBe(false);
    expect(client.calls[0].params).toEqual({ ids: ['z2'] });
    const toasts = TestBed.inject(PhxToastService).toasts();
    expect(toasts).toHaveLength(1);
    expect(toasts[0].status).toBe('danger');
    expect(toasts[0].title).toBe('Delete');
    expect(toasts[0].text).toBe('You are not allowed to do this (request req-9)');
  });

  it('an action without a confirm posts at once with $id substituted from the row', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'dns.zone.sign': ok({ signed: true }) },
    });
    const sign: Action = {
      id: 'sign',
      role: 'button',
      function: 'dns.zone.sign',
      params: { id: '$id', algorithm: 'ecdsa' },
    };
    const outcome = await (
      await runner(client)
    ).run({
      descriptor: LIST,
      action: sign,
      rows: [ZONES[1]],
    });
    expect(dialogs()).toHaveLength(0);
    expect(outcome?.ok).toBe(true);
    expect(client.calls[0].params).toEqual({ id: 'z2', algorithm: 'ecdsa' });
  });

  it('the route /f/<func> of a kind: action descriptor runs the same runner with $id from the route parameters', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [ENABLE],
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'keel.product.enable': ok({ id: 'dns', state: 'enabled', changed: true, restart: false }),
      },
    });
    TestBed.configureTestingModule({
      providers: [
        providePhoenixIcons(),
        provideRouter(
          [{ path: 'f/:func', component: KeelScreenComponent }],
          withComponentInputBinding(),
        ),
        { provide: KeelClient, useValue: client },
      ],
    });
    await TestBed.inject(KeelBoot).load();
    await TestBed.inject(KeelI18n).load('en');
    const harness = await RouterTestingHarness.create();
    const screen = await harness.navigateByUrl(
      '/f/keel.product.enable?id=dns',
      KeelScreenComponent,
    );
    await harness.fixture.whenStable();
    harness.detectChanges();
    expect(screen.screenParams()).toEqual({ id: 'dns' });
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('keel-action')?.getAttribute('data-phase')).toBe('running');

    const open = dialog();
    expect(open.querySelector('.title')?.textContent).toBe('Enable');
    expect(client.calls).toHaveLength(0);
    const done = new Promise<Envelope>((resolve) => screen.done.subscribe(resolve));
    confirmButton().click();
    expect((await done).ok).toBe(true);
    harness.detectChanges();
    expect(client.calls).toHaveLength(1);
    expect(client.calls[0].func).toBe('keel.product.enable');
    expect(client.calls[0].params).toEqual({ id: 'dns' });
    expect(client.calls[0].options.idempotencyKey).toMatch(UUID);
    expect(element.querySelector('keel-action')?.getAttribute('data-phase')).toBe('done');
    expect(element.querySelector('phx-banner')?.textContent?.trim()).toBe('Enable completed');
    expect(
      [...element.querySelectorAll('phx-prop')].map((prop) => [
        prop.querySelector('.label-text')?.textContent,
        prop.querySelector('.value-text')?.textContent?.trim(),
      ]),
    ).toEqual([
      ['Product', 'dns'],
      ['State', 'Enabled'],
      ['keel.product.changed', 'keel.value.true'],
      ['keel.product.restart', 'keel.value.false'],
    ]);
  });
});

describe('the action body', () => {
  it('carries ids for delete and group and the substituted params for a button', () => {
    expect(bodyOf({ descriptor: LIST, action: action('delete'), rows: ZONES })).toEqual({
      ids: ['z1', 'z2'],
    });
    expect(bodyOf({ descriptor: LIST, action: action('suspend'), rows: [ZONES[1]] })).toEqual({
      ids: ['z2'],
    });
    const enable = routeAction(ENABLE, { id: 'dns' });
    expect(enable).toEqual({
      id: 'enable',
      role: 'button',
      function: 'keel.product.enable',
      params: { id: 'dns' },
      confirm: true,
    });
    expect(bodyOf({ descriptor: ENABLE, action: enable })).toEqual({ id: 'dns' });
    expect(substitute({ id: '$id', zone: '$id', ttl: 300 }, 'z9')).toEqual({
      id: 'z9',
      zone: 'z9',
      ttl: 300,
    });
    expect(substitute({ id: '$id' }, undefined)).toEqual({ id: '$id' });
  });
});
