import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelBoot, KeelCallError, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelScreenComponent, paramsOf } from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { convertToParamMap, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';
import { throwError, type Observable } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const OPTIONS = load<Descriptor>('descriptors/keel.account.options.json');
const GET = load<Descriptor>('descriptors/keel.account.get.json');
const BOOT = load<Boot>('boot.json');
const ID = '01J8A0000000000000000000Z1';

const MESSAGES = {
  dns: { zone: { edit: { title: 'Zone' }, list: { title: 'Zones' }, name: 'Zone name' } },
  keel: {
    action: { save: 'Save' },
    error: {
      not_found: 'Not found',
      forbidden: 'You are not allowed to do this',
      function_not_found: 'The function {function} does not exist',
      request: '{message} (request {id})',
    },
    screen: { unsupported: 'Screens of kind {kind} are not rendered yet' },
  },
};

function ok(data: unknown, requestId = 'req-ok'): Envelope {
  return { ok: true, data, meta: { requestId } };
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

class ForbiddingClient extends KeelMockClient {
  override descriptor(func: string): Observable<Descriptor> {
    return throwError(
      () =>
        new KeelCallError({
          code: 'forbidden',
          key: 'keel.error.forbidden',
          requestId: `req-${func}`,
        }),
    );
  }
}

async function render(
  func: string,
  client: KeelMockClient,
  params?: Record<string, unknown>,
): Promise<ComponentFixture<KeelScreenComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', func);
  if (params !== undefined) {
    fixture.componentRef.setInput('params', params);
  }
  fixture.detectChanges();
  await settle(fixture);
  await settle(fixture);
  return fixture;
}

describe('keel-screen', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('a form descriptor renders keel-form with the values read through ui.read and hands a save back', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [EDIT],
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'dns.zone.get': ok({ id: ID, name: 'a.org', kind: 'master', ttl: 300 }),
        'dns.zone.edit': ok({ id: ID }, 'req-saved'),
      },
    });
    const fixture = await render('dns.zone.edit', client, { id: ID });
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('screen');
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['dns.zone.get', { id: ID }],
    ]);
    const form = element.querySelector('keel-form');
    expect(form?.getAttribute('data-form')).toBe('dns.zone.edit');
    expect(form?.getAttribute('data-mode')).toBe('edit');
    expect(element.querySelector('phx-form-sheet h1')?.textContent).toBe('Zone');
    expect(element.querySelector<HTMLInputElement>('phx-input input')?.value).toBe('a.org');

    const done = jest.fn();
    const closed = jest.fn();
    fixture.componentInstance.done.subscribe(done);
    fixture.componentInstance.closed.subscribe(closed);
    element.querySelector<HTMLButtonElement>('button.save')?.click();
    await settle(fixture);
    expect(client.calls[1].func).toBe('dns.zone.edit');
    expect(done).toHaveBeenCalledTimes(1);
    expect(done.mock.calls[0][0]).toEqual(ok({ id: ID }, 'req-saved'));

    element.querySelector<HTMLButtonElement>('phx-form-sheet .header__close')?.click();
    await fixture.whenStable();
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it('a list descriptor renders keel-list with the screen parameters as the scope', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [LIST],
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'dns.zone.list': ok({ data: [{ id: ID, name: 'a.org' }], total: 1, totals: null }),
        'keel.account.options': ok({ data: [], more: false }),
      },
    });
    const fixture = await render('dns.zone.list', client, { view: 'mine' });
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('screen');
    expect(element.querySelector('keel-list')?.getAttribute('data-list')).toBe('dns.zone.list');
    expect(element.querySelector('phx-page-header .title__text')?.textContent).toBe('Zones');
    expect(client.callsOf('dns.zone.list').map((call) => call.params)).toEqual([
      { page: 1, pageSize: 25, scope: { view: 'mine' } },
    ]);
    expect(client.callsOf('keel.account.options').map((call) => call.params)).toEqual([
      { scope: { view: 'mine' }, limit: 20 },
    ]);
    expect(element.querySelector('.body .row a.link')?.textContent).toBe('a.org');
  });

  it('a kind without a renderer yet shows phx-not-found naming the kind', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [OPTIONS],
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    const fixture = await render('keel.account.options', client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('screen');
    expect(element.querySelector('phx-not-found .description')?.textContent).toBe(
      'Screens of kind options are not rendered yet',
    );
    expect(client.calls).toHaveLength(0);
  });

  it('a function the server does not know shows phx-not-found with its message', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    const fixture = await render('dns.nothing', client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('failed');
    expect(element.querySelector('phx-not-found .title')?.textContent).toBe('Not found');
    expect(element.querySelector('phx-not-found .description')?.textContent).toBe(
      'The function dns.nothing does not exist',
    );
  });

  it('a descriptor the caller may not read shows phx-error with the request id', async () => {
    const client = new ForbiddingClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    const fixture = await render('dns.zone.edit', client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('failed');
    expect(element.querySelector('phx-not-found')).toBeNull();
    expect(element.querySelector('phx-error .text')?.textContent?.trim()).toBe(
      'You are not allowed to do this (request req-dns.zone.edit)',
    );
  });

  it('a query member named params stays a query member instead of replacing the screen parameters', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [GET],
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'keel.account.get': ok({ id: ID, kind: 'client' }) },
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
      `/f/keel.account.get?params=x&id=${ID}`,
      KeelScreenComponent,
    );
    await settle(harness.fixture);
    expect(screen.params()).toBe('x');
    expect(screen.screenParams()).toEqual({ params: 'x', id: ID });
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['keel.account.get', { params: 'x', id: ID }],
    ]);
  });

  it('paramsOf turns the query of the route into screen parameters', () => {
    expect(paramsOf(convertToParamMap({ id: 'z1', tag: ['a', 'b'] }))).toEqual({
      id: 'z1',
      tag: ['a', 'b'],
    });
    expect(paramsOf(convertToParamMap({}))).toEqual({});
  });
});
