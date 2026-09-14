import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Level } from '@globalart/keel/meta';
import {
  KeelLayoutComponent,
  KeelRailComponent,
  brandOf,
  keelProviders,
  levelKey,
  levelLabel,
  sidebarOf,
} from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

const MESSAGES = {
  menu: { dns: 'DNS', hosting: 'Hosting', mail: 'Mail', system: 'System' },
  product: {
    dns: { brand: 'KeelDNS', level: { reseller: 'Partner' } },
    panel: { brand: 'KeelPanel' },
  },
  dns: { zone: { list: { title: 'Zones' } } },
  hosting: { site: { list: { title: 'Sites' } } },
  mail: { box: { list: { title: 'Mailboxes' } } },
  keel: {
    brand: 'Keel',
    level: { reseller: 'Reseller', owner: 'Owner' },
    account: { list: { title: 'Accounts' } },
    principal: { list: { title: 'Principals' } },
    product: { list: { title: 'Products' } },
  },
};

function load(name: string): Boot {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as Boot;
}

function twoProducts(): Boot {
  return load('boot.json');
}

function noProduct(): Boot {
  return load('boot-none.json');
}

function oneProduct(): Boot {
  const boot = twoProducts();
  const [dns] = boot.products;
  boot.products = [dns];
  boot.brand = dns.brand;
  boot.menu = boot.menu.filter((group) => group.product !== 'panel');
  return boot;
}

function withoutVisibleItem(): Boot {
  const boot = twoProducts();
  boot.menu = boot.menu.filter((group) => group.product !== 'panel');
  return boot;
}

function withSecondGroup(): Boot {
  const boot = twoProducts();
  boot.menu.push({
    id: 'mail',
    product: 'panel',
    label: 'menu.mail',
    icon: 'mail',
    items: [
      { path: 'mail/boxes', label: 'mail.box.list.title', func: 'mail.box.list', kind: 'list' },
    ],
  });
  return boot;
}

async function render(boot: Boot): Promise<ComponentFixture<KeelLayoutComponent>> {
  const client = new KeelMockClient({ boot, catalogue: { lang: 'en', messages: MESSAGES } });
  TestBed.configureTestingModule({
    providers: [...keelProviders(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelLayoutComponent);
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  return fixture;
}

interface ShownGroup {
  id: string;
  label: string | null;
  items: string[];
}

interface ShownSection {
  product: string | null;
  label: string | null;
  groups: ShownGroup[];
}

function eyebrowOf(group: HTMLElement): string | null {
  return group.querySelector(':scope > .eyebrow')?.textContent?.trim() ?? null;
}

function sectionsOf(fixture: ComponentFixture<unknown>): ShownSection[] {
  const element: HTMLElement = fixture.nativeElement;
  return [...element.querySelectorAll<HTMLElement>('phx-sidebar-group.section')].map((section) => ({
    product: section.dataset['product'] ?? null,
    label: eyebrowOf(section),
    groups: [...section.querySelectorAll<HTMLElement>('phx-sidebar-group.group')].map((group) => ({
      id: group.dataset['group'] ?? '',
      label: eyebrowOf(group),
      items: [...group.querySelectorAll('phx-sidebar-item a')].map(
        (item) => `${item.querySelector('.label')?.textContent} ${item.getAttribute('href')}`,
      ),
    })),
  }));
}

function brandShown(fixture: ComponentFixture<unknown>): {
  alt: string | null;
  text: string | null;
} {
  const element: HTMLElement = fixture.nativeElement;
  const logo = element.querySelector('phx-brand-logo');
  return {
    alt: logo?.querySelector('img')?.getAttribute('alt') ?? null,
    text: logo?.querySelector('.brand')?.textContent?.trim() ?? null,
  };
}

function levelShown(fixture: ComponentFixture<unknown>): string | null {
  const rail = fixture.debugElement.query(By.directive(KeelRailComponent))
    .componentInstance as KeelRailComponent;
  return rail.level();
}

const SYSTEM: ShownGroup = {
  id: 'system',
  label: 'System',
  items: [
    'Accounts /f/keel.account.list',
    'Principals /f/keel.principal.list',
    'Products /f/keel.product.list',
  ],
};
const DNS: ShownGroup = { id: 'dns', label: 'DNS', items: ['Zones /f/dns.zone.list'] };
const HOSTING: ShownGroup = {
  id: 'hosting',
  label: 'Hosting',
  items: ['Sites /f/hosting.site.list'],
};
const MAIL: ShownGroup = { id: 'mail', label: 'Mail', items: ['Mailboxes /f/mail.box.list'] };
const PLATFORM: ShownSection = { product: null, label: null, groups: [SYSTEM] };

describe('the sidebar rule', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('one product shows its groups then system under the product brand', async () => {
    const fixture = await render(oneProduct());
    expect(sectionsOf(fixture)).toEqual([{ product: 'dns', label: null, groups: [DNS] }, PLATFORM]);
    expect(brandShown(fixture)).toEqual({ alt: 'KeelDNS', text: null });
    expect(fixture.nativeElement.querySelector('phx-brand-logo img')?.getAttribute('src')).toBe(
      '/brand/dns/logo.svg',
    );
    expect(fixture.nativeElement.querySelector('phx-brand-logo .mark')?.textContent).toBe('K');
    expect(levelShown(fixture)).toBe('Partner');
  });

  it('two products show a heading per product in boot order with the platform group last and the platform brand', async () => {
    const boot = twoProducts();
    boot.menu.reverse();
    const fixture = await render(boot);
    expect(sectionsOf(fixture)).toEqual([
      { product: 'dns', label: 'KeelDNS', groups: [DNS] },
      { product: 'panel', label: 'KeelPanel', groups: [HOSTING] },
      PLATFORM,
    ]);
    expect(brandShown(fixture)).toEqual({ alt: null, text: 'Keel' });
    expect(fixture.nativeElement.querySelector('phx-brand-logo img')).toBeNull();
    expect(levelShown(fixture)).toBe('Reseller');
  });

  it('a product with several groups keeps them under its heading in menu order', async () => {
    const boot = withSecondGroup();
    boot.menu.reverse();
    const fixture = await render(boot);
    expect(sectionsOf(fixture)).toEqual([
      { product: 'dns', label: 'KeelDNS', groups: [DNS] },
      { product: 'panel', label: 'KeelPanel', groups: [MAIL, HOSTING] },
      PLATFORM,
    ]);
  });

  it('no product shows the platform group alone', async () => {
    const fixture = await render(noProduct());
    expect(sectionsOf(fixture)).toEqual([PLATFORM]);
    expect(brandShown(fixture)).toEqual({ alt: null, text: 'Keel' });
    expect(levelShown(fixture)).toBe('Owner');
  });

  it('a heading without a visible item is omitted', async () => {
    const fixture = await render(withoutVisibleItem());
    expect(sectionsOf(fixture).map((section) => section.product)).toEqual(['dns', null]);
    expect(sectionsOf(fixture).map((section) => section.label)).toEqual(['KeelDNS', null]);
    expect(brandShown(fixture)).toEqual({ alt: null, text: 'Keel' });
  });

  it('the level label key switches between product.<id>.level.* and keel.level.*', () => {
    const one = oneProduct();
    const two = twoProducts();
    const level: Level = 'reseller';
    expect(levelKey(one.products, level)).toBe('product.dns.level.reseller');
    expect(levelKey(two.products, level)).toBe('keel.level.reseller');
    expect(levelKey([], 'owner')).toBe('keel.level.owner');
    const label = (key: string) => `<${key}>`;
    expect(levelLabel(one.products, level, MESSAGES, label)).toBe('Partner');
    expect(levelLabel(one.products, 'admin', MESSAGES, label)).toBe('<keel.level.admin>');
    expect(levelLabel(two.products, level, MESSAGES, label)).toBe('Reseller');
  });

  it('sidebarOf and brandOf are pure over the boot payload', () => {
    const label = (key: string) => key;
    const shape = (boot: Boot) =>
      sidebarOf(boot, label).map((section) => [
        section.product,
        section.label,
        section.groups.map((group) => group.label),
      ]);
    expect(shape(noProduct())).toEqual([[null, null, ['menu.system']]]);
    expect(shape(oneProduct())).toEqual([
      ['dns', null, ['menu.dns']],
      [null, null, ['menu.system']],
    ]);
    expect(shape(withSecondGroup())).toEqual([
      ['dns', 'product.dns.brand', ['menu.dns']],
      ['panel', 'product.panel.brand', ['menu.hosting', 'menu.mail']],
      [null, null, ['menu.system']],
    ]);
    const empty = twoProducts();
    empty.menu = [];
    expect(sidebarOf(empty, label)).toEqual([]);
    expect(brandOf({ name: 'keel.brand', hue: 220, chroma: 0.08 }, () => 'keel')).toEqual({
      name: 'keel',
      logo: null,
      mark: 'K',
    });
    expect(brandOf({ name: 'x', logo: '/brand/logo.svg', hue: 1, chroma: 0.1 }, () => '')).toEqual({
      name: '',
      logo: '/brand/logo.svg',
      mark: '',
    });
  });
});
