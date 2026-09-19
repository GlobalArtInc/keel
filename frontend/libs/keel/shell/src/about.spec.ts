import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelClient } from '@globalart/keel/client';
import type { Boot, Messages } from '@globalart/keel/meta';
import { keelProviders, productViews } from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxDrawerService } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

const EN: Messages = {
  menu: { system: 'System', dns: 'DNS', hosting: 'Hosting' },
  product: { dns: { brand: 'KeelDNS' }, panel: { brand: 'KeelPanel' } },
  dns: { zone: { list: { title: 'Zones' } } },
  hosting: { site: { list: { title: 'Sites' } } },
  keel: {
    brand: 'Keel',
    level: { reseller: 'Reseller' },
    shell: {
      rail: { notifications: 'Notifications', tasks: 'Tasks', help: 'Help' },
      about: {
        title: 'About this installation',
        version: 'Version',
        api: 'API version',
        products: 'Products',
        no_products: 'No product is enabled',
        edition: 'Edition {edition}',
      },
      user: { language: 'Language', theme: 'Theme' },
    },
  },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

describe('the about drawer', () => {
  afterEach(() => {
    TestBed.inject(PhxDrawerService).closeAll();
    TestBed.resetTestingModule();
  });

  it('the help item opens the about drawer with the version, the API version and the products of the boot', async () => {
    const boot = load<Boot>('boot.json');
    boot.products[0].edition = 'pro';
    TestBed.configureTestingModule({
      providers: [
        ...keelProviders(),
        {
          provide: KeelClient,
          useValue: new KeelMockClient({
            boot,
            descriptors: [],
            catalogue: { lang: 'en', messages: EN },
          }),
        },
      ],
    });
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/f/dns.zone.list');
    await harness.fixture.whenStable();
    harness.detectChanges();
    const element: HTMLElement = harness.fixture.nativeElement;

    element.querySelector<HTMLElement>('phx-rail-item[data-rail="help"] button')?.click();
    await harness.fixture.whenStable();
    harness.detectChanges();
    const about = element.ownerDocument.querySelector('keel-about');
    expect(about).not.toBeNull();
    expect(about?.querySelector('phx-drawer-header .title')?.textContent?.trim()).toBe(
      'About this installation',
    );
    const props = [...(about?.querySelectorAll('phx-prop-list phx-prop') ?? [])].map((prop) =>
      prop.textContent?.replace(/\s+/g, ' ').trim(),
    );
    expect(props).toEqual([`Version ${boot.keel.version}`, `API version ${boot.keel.api}`]);
    const products = [...(about?.querySelectorAll('.product') ?? [])].map((product) => ({
      id: product.getAttribute('data-product'),
      name: product.querySelector('.product__name')?.textContent?.trim(),
      edition: product.querySelector('.product__edition')?.textContent?.trim() ?? null,
    }));
    expect(products).toEqual([
      { id: 'dns', name: 'KeelDNS', edition: 'Edition pro' },
      { id: 'panel', name: 'KeelPanel', edition: null },
    ]);
    expect(productViews([], (key) => key)).toEqual([]);
  });
});
