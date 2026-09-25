import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { Component, makeEnvironmentProviders, type EnvironmentProviders } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  KeelBoot,
  KeelClient,
  KeelI18n,
  type Catalogue,
  type Fetched,
} from '@globalart/keel/client';
import { injectKeelWidgets, provideKeelWidgets, type KeelWidget } from '@globalart/keel/forms';
import type { Boot, Envelope } from '@globalart/keel/meta';
import { injectKeelViewContext, provideKeelViews } from '@globalart/keel/renderer';
import {
  KeelProducts,
  KeelStartup,
  keelProviders,
  provideKeelProducts,
  type KeelProduct,
} from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';
import { of, type Observable } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const TAG_CLOUD = 'note.tag.list';
const WIDGET_LIST = 'note.widget.list';
const NOTE_LIST = 'note.list';

const MESSAGES = {
  menu: { system: 'System', demo: 'Demo' },
  keel: {
    brand: 'Keel',
    account: { list: { title: 'Accounts' } },
    shell: {
      not_found: {
        title: 'Page not found',
        text: 'The address {path} does not exist in this installation',
        home: 'Go to the start page',
      },
    },
  },
  note: { list: { title: 'Notes' }, tag: { list: { title: 'Tags' } } },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function demoBoot(): Boot {
  const boot: Boot = { ...load<Boot>('boot-none.json'), dashboard: [] };
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
    icon: 'notebook',
    items: [
      {
        path: 'demo/notes',
        label: 'note.list.title',
        func: NOTE_LIST,
        kind: 'list',
        icon: 'notebook',
      },
      { path: 'demo/tags', label: 'note.tag.list.title', func: TAG_CLOUD, icon: 'tag' },
    ],
  });
  boot.views = [TAG_CLOUD, WIDGET_LIST];
  return boot;
}

function guestBoot(): Boot {
  const boot = demoBoot();
  boot.principal = null;
  boot.menu = [];
  boot.views = [];
  return boot;
}

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-shell' } };
}

const SERVED = ['en', 'ru'];

class SpeakingClient extends KeelMockClient {
  override catalogue(lang: string): Observable<Fetched<Catalogue> | null> {
    const served = SERVED.includes(lang) ? lang : 'en';
    return of({ body: { lang: served, messages: { ...MESSAGES, served } }, etag: null });
  }
}

@Component({ selector: 'keel-test-tag-cloud', template: '<p class="cloud">{{ func }}</p>' })
class TagCloudComponent {
  readonly func = injectKeelViewContext().descriptor?.name ?? TAG_CLOUD;
}

@Component({ selector: 'keel-test-other-view', template: '<p class="other"></p>' })
class OtherViewComponent {}

@Component({ selector: 'keel-test-widget', template: '' })
class WidgetComponent {}

@Component({ selector: 'keel-test-widget-list', template: '<p class="widgets">{{ names }}</p>' })
class WidgetListComponent {
  readonly names = injectKeelWidgets()
    .map((widget) => widget.name)
    .join(' ');
}

const SHELL_WIDGET: KeelWidget = { name: 'shell-widget', component: WidgetComponent };
const PRODUCT_WIDGET: KeelWidget = { name: 'product-widget', component: WidgetComponent };

interface Harnessed {
  harness: RouterTestingHarness;
  router: Router;
  client: KeelMockClient;
  demo: jest.Mock<Promise<EnvironmentProviders>, []>;
  other: jest.Mock<Promise<EnvironmentProviders>, []>;
}

async function start(
  boot: Boot,
  answers: Record<string, Envelope> = {},
  client = new KeelMockClient({ boot, catalogue: { lang: 'en', messages: MESSAGES }, answers }),
  widgets: readonly KeelWidget[] = [],
): Promise<Harnessed> {
  const demo = jest.fn(async () =>
    makeEnvironmentProviders([
      provideKeelViews([
        { func: TAG_CLOUD, component: TagCloudComponent },
        { func: WIDGET_LIST, component: WidgetListComponent },
      ]),
      provideKeelWidgets([PRODUCT_WIDGET]),
    ]),
  );
  const other = jest.fn(async () =>
    provideKeelViews([{ func: 'hosting.file-manager', component: OtherViewComponent }]),
  );
  const products: KeelProduct[] = [
    { id: 'demo', load: demo },
    { id: 'panel', load: other },
  ];
  TestBed.configureTestingModule({
    providers: [...keelProviders({ products, widgets }), { provide: KeelClient, useValue: client }],
  });
  const harness = await RouterTestingHarness.create();
  return { harness, router: TestBed.inject(Router), client, demo, other };
}

async function settle(harness: RouterTestingHarness): Promise<void> {
  await harness.fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  harness.detectChanges();
}

describe('bootstrapKeel', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('with demo and a second product in products, a boot naming demo alone resolves one load() and registers its views, and the other is never called', async () => {
    const { harness, router, demo, other } = await start(demoBoot());
    await harness.navigateByUrl(`/f/${TAG_CLOUD}`);
    await settle(harness);
    expect(demo).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('keel-layout')).not.toBeNull();
    expect(element.querySelector('keel-screen')?.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('keel-test-tag-cloud .cloud')?.textContent).toBe(TAG_CLOUD);
    expect(router.url).toBe(`/f/${TAG_CLOUD}`);

    await harness.navigateByUrl('/f/keel.account.list');
    await settle(harness);
    expect(demo).toHaveBeenCalledTimes(1);
    await expect(TestBed.inject(KeelProducts).load(demoBoot())).resolves.toHaveLength(1);
  });

  it('the chunks of the enabled products are fetched after /boot and before any screen route, and a chunk that does not load fails the startup until a retry', async () => {
    const { harness, router, demo, other } = await start(demoBoot());
    const startup = TestBed.inject(KeelStartup);
    demo.mockRejectedValueOnce(new Error('chunk lost'));
    await expect(harness.navigateByUrl('/login')).resolves.toBeNull();
    expect(router.url).toBe('/');
    expect(startup.state()).toBe('failed');
    expect(startup.failure()).toEqual(new Error('chunk lost'));
    expect(demo).toHaveBeenCalledTimes(1);
    await expect(startup.ready()).rejects.toThrow('chunk lost');
    expect(demo).toHaveBeenCalledTimes(1);

    startup.reset();
    await harness.navigateByUrl('/login');
    await settle(harness);
    expect(router.url).toBe('/login');
    expect(startup.state()).toBe('ready');
    expect(demo).toHaveBeenCalledTimes(2);
    expect(other).not.toHaveBeenCalled();
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('keel-login')).not.toBeNull();

    await harness.navigateByUrl(`/f/${TAG_CLOUD}`);
    await settle(harness);
    expect(demo).toHaveBeenCalledTimes(2);
    expect(element.querySelector('keel-test-tag-cloud .cloud')?.textContent).toBe(TAG_CLOUD);
  });

  it('the widgets of bootstrapKeel and those a product provides are both visible to the screens, and the product widgets to nothing else', async () => {
    const { harness } = await start(demoBoot(), {}, undefined, [SHELL_WIDGET]);
    await harness.navigateByUrl(`/f/${WIDGET_LIST}`);
    await settle(harness);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('keel-screen')?.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('keel-test-widget-list .widgets')?.textContent).toBe(
      'shell-widget product-widget',
    );
    expect(TestBed.runInInjectionContext(injectKeelWidgets)).toEqual([SHELL_WIDGET]);
  });

  it('refuses a product registered twice', () => {
    expect(() =>
      provideKeelProducts([
        { id: 'demo', load: () => Promise.reject(new Error('unused')) },
        { id: 'demo', load: () => Promise.reject(new Error('unused')) },
      ]),
    ).toThrow('the product demo is registered twice');
  });

  it('the guard loads /boot once, reads the catalogue for the principal and sends a guest to /login with returnTo', async () => {
    const { harness, router, demo, client } = await start(guestBoot());
    const boot = TestBed.inject(KeelBoot);
    const loads = jest.spyOn(client, 'boot');
    await harness.navigateByUrl(`/f/${TAG_CLOUD}?tab=all`);
    await settle(harness);
    expect(router.url).toBe(`/login?returnTo=%2Ff%2F${TAG_CLOUD}%3Ftab%3Dall`);
    expect(loads).toHaveBeenCalledTimes(1);
    expect(boot.loaded()).toBe(true);
    expect(TestBed.inject(KeelI18n).lang()).toBe('en');
    expect(demo).toHaveBeenCalledTimes(1);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('keel-login')).not.toBeNull();
    expect(element.querySelector('keel-layout')).toBeNull();

    await harness.navigateByUrl('/');
    await settle(harness);
    expect(router.url).toBe('/login');
    expect(loads).toHaveBeenCalledTimes(1);
  });

  it('after boot the shell loads the catalogue for principal.lang once, and a language the server lacks is asked for once', async () => {
    const spoken = demoBoot();
    if (spoken.principal) {
      spoken.principal.lang = 'ru';
    }
    const { harness, client } = await start(spoken, {}, new SpeakingClient({ boot: spoken }));
    const catalogues = jest.spyOn(client, 'catalogue');
    await harness.navigateByUrl(`/f/${TAG_CLOUD}`);
    await settle(harness);
    expect(TestBed.inject(KeelI18n).lang()).toBe('ru');
    expect(catalogues.mock.calls.map(([lang]) => lang)).toEqual(['ru']);
    await harness.navigateByUrl('/f/keel.account.list');
    await harness.navigateByUrl('/');
    await settle(harness);
    expect(catalogues).toHaveBeenCalledTimes(1);
    TestBed.resetTestingModule();

    const unserved = demoBoot();
    if (unserved.principal) {
      unserved.principal.lang = 'de';
    }
    const second = await start(unserved, {}, new SpeakingClient({ boot: unserved }));
    const asked = jest.spyOn(second.client, 'catalogue');
    await second.harness.navigateByUrl(`/f/${TAG_CLOUD}`);
    await second.harness.navigateByUrl('/');
    await settle(second.harness);
    expect(TestBed.inject(KeelI18n).lang()).toBe('en');
    expect(asked.mock.calls.map(([lang]) => lang)).toEqual(['de']);
  });

  it('/ goes to the first item of the menu and anything else shows the not-found page with the path', async () => {
    const { harness, router } = await start(demoBoot(), {
      [NOTE_LIST]: ok({ data: [], total: 0 }),
    });
    await harness.navigateByUrl('/');
    await settle(harness);
    expect(router.url).toBe(`/f/${NOTE_LIST}`);

    await harness.navigateByUrl('/nowhere/at/all?x=1');
    await settle(harness);
    expect(router.url).toBe('/nowhere/at/all?x=1');
    const element: HTMLElement = harness.fixture.nativeElement;
    const notFound = element.querySelector('keel-not-found');
    expect(notFound?.getAttribute('data-path')).toBe('/nowhere/at/all');
    expect(notFound?.querySelector('.title')?.textContent).toBe('Page not found');
    expect(notFound?.querySelector('.description')?.textContent).toBe(
      'The address /nowhere/at/all does not exist in this installation',
    );
    const home = notFound?.querySelector<HTMLButtonElement>('button');
    expect(home?.textContent?.trim()).toBe('Go to the start page');
    home?.click();
    await settle(harness);
    expect(router.url).toBe(`/f/${NOTE_LIST}`);
  });

  it('/logout posts auth.logout, reloads the boot and lands on /login', async () => {
    const { harness, router, client } = await start(demoBoot(), {
      'auth.logout': ok({ revoked: true }),
    });
    const loads = jest.spyOn(client, 'boot');
    await harness.navigateByUrl(`/f/${TAG_CLOUD}`);
    await settle(harness);
    expect(loads).toHaveBeenCalledTimes(1);
    await harness.navigateByUrl('/logout');
    await settle(harness);
    expect(client.calls.map((call) => call.func)).toEqual(['auth.logout']);
    expect(loads).toHaveBeenCalledTimes(2);
    expect(router.url).toBe('/login');
    expect(TestBed.inject(KeelStartup).state()).toBe('ready');
  });

  it('a refused logout drops the boot so the next route loads it again', async () => {
    const refused: Envelope = {
      ok: false,
      error: { code: 'unauthenticated', key: 'keel.error.unauthenticated' },
    };
    const { harness, router, client } = await start(demoBoot(), { 'auth.logout': refused });
    const loads = jest.spyOn(client, 'boot');
    await harness.navigateByUrl(`/f/${TAG_CLOUD}`);
    await settle(harness);
    expect(loads).toHaveBeenCalledTimes(1);
    await harness.navigateByUrl('/logout');
    await settle(harness);
    expect(router.url).toBe('/login');
    expect(loads).toHaveBeenCalledTimes(2);
    expect(TestBed.inject(KeelBoot).loaded()).toBe(true);
  });
});
