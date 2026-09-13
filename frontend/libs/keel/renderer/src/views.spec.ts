import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { Component } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor } from '@globalart/keel/meta';
import {
  KEEL_VIEWS,
  KeelScreenComponent,
  injectKeelViewContext,
  injectKeelViews,
  productOf,
  provideKeelViews,
  viewOf,
  type KeelViewContext,
} from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxTabsService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const BOOT = load<Boot>('boot.json');
const FILE_MANAGER = 'hosting.file-manager';

const MESSAGES = {
  product: { panel: { brand: 'KeelPanel' } },
  keel: {
    error: { not_found: 'Not found' },
    view: { missing: 'The screen {view} belongs to {product}, whose views are not in this shell' },
  },
};

function bootWithView(): Boot {
  const boot = load<Boot>('boot.json');
  boot.views = [FILE_MANAGER];
  boot.menu[1].items.push({
    path: 'hosting/files',
    label: 'hosting.file-manager.title',
    func: FILE_MANAGER,
  });
  return boot;
}

const received: KeelViewContext[] = [];

@Component({
  selector: 'keel-test-zone-board',
  template: '<p class="board">{{ context.descriptor?.name ?? "no descriptor" }}</p>',
})
class ZoneBoardComponent {
  readonly context = injectKeelViewContext();

  constructor() {
    received.push(this.context);
  }
}

@Component({ selector: 'keel-test-file-manager', template: '<p class="files"></p>' })
class FileManagerComponent {
  readonly context = injectKeelViewContext();
}

async function render(
  func: string,
  client: KeelMockClient,
  views: Parameters<typeof provideKeelViews>[0],
  params?: Record<string, unknown>,
): Promise<ComponentFixture<KeelScreenComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [
      providePhoenixIcons(),
      provideKeelViews(views),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', func);
  if (params !== undefined) {
    fixture.componentRef.setInput('params', params);
  }
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  return fixture;
}

describe('provideKeelViews', () => {
  afterEach(() => {
    received.length = 0;
    TestBed.resetTestingModule();
  });

  it('registers components under function names and hands them to the screen', () => {
    TestBed.configureTestingModule({
      providers: [
        provideKeelViews([{ func: 'dns.zone.list', component: ZoneBoardComponent }]),
        provideKeelViews([{ func: FILE_MANAGER, component: FileManagerComponent }]),
      ],
    });
    const views = TestBed.runInInjectionContext(injectKeelViews);
    expect(views.map((view) => view.func)).toEqual(['dns.zone.list', FILE_MANAGER]);
    expect(viewOf(views, FILE_MANAGER)?.component).toBe(FileManagerComponent);
    expect(viewOf(views, 'dns.record.list')).toBeUndefined();
    expect(TestBed.inject(KEEL_VIEWS)).toHaveLength(2);
  });

  it('refuses a function registered twice in one call', () => {
    expect(() =>
      provideKeelViews([
        { func: 'dns.zone.list', component: ZoneBoardComponent },
        { func: 'dns.zone.list', component: FileManagerComponent },
      ]),
    ).toThrow('the view dns.zone.list is registered twice');
  });

  it('a component registered through provideKeelViews replaces the generic screen and receives the context with the descriptor', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [LIST],
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    const fixture = await render('dns.zone.list', client, [
      { func: 'dns.zone.list', component: ZoneBoardComponent },
    ]);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('keel-test-zone-board .board')?.textContent).toBe('dns.zone.list');
    expect(element.querySelector('phx-data-table')).toBeNull();
    expect(received).toHaveLength(1);
    const context = received[0];
    expect(context.descriptor).toEqual(LIST);
    expect(context.params).toEqual({});
    expect(context.client).toBe(client);
    expect(context.principal?.login).toBe('res1');
    expect(context.tabs).toBe(TestBed.inject(PhxTabsService));
  });

  it('a view of boot.views is created without a descriptor and with the screen parameters', async () => {
    const client = new KeelMockClient({
      boot: bootWithView(),
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    const fixture = await render(
      FILE_MANAGER,
      client,
      [{ func: FILE_MANAGER, component: ZoneBoardComponent }],
      { path: '/var/www' },
    );
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('keel-test-zone-board .board')?.textContent).toBe('no descriptor');
    expect(received[0].descriptor).toBeUndefined();
    expect(received[0].params).toEqual({ path: '/var/www' });
  });

  it('a view of boot.views without a component shows phx-not-found naming the product', async () => {
    const client = new KeelMockClient({
      boot: bootWithView(),
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    const fixture = await render(FILE_MANAGER, client, []);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('missing');
    const notFound = element.querySelector('phx-not-found');
    expect(notFound?.querySelector('.title')?.textContent).toBe('Not found');
    expect(notFound?.querySelector('.description')?.textContent).toBe(
      'The screen hosting.file-manager belongs to KeelPanel, whose views are not in this shell',
    );
    expect(notFound?.querySelector('button')).toBeNull();
    expect(received).toHaveLength(0);
  });

  it('productOf reads the product from the menu group and falls back to the module', () => {
    const boot = bootWithView();
    expect(productOf(boot, FILE_MANAGER)).toBe('panel');
    expect(productOf(boot, 'dns.zone.list')).toBe('dns');
    expect(productOf(boot, 'keel.account.list')).toBe('keel');
    expect(productOf(boot, 'billing.invoice.list')).toBe('billing');
    expect(productOf(null, 'billing.invoice.list')).toBe('billing');
  });
});
