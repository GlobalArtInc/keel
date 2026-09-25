import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { DNS_BRAND, DNS_ICON, DNS_ID, DNS_VIEWS, provideKeelDns } from '@globalart/keel-dns';
import { isIconName } from '@globalart/keel/forms';
import { lookup } from '@globalart/keel/meta';
import { injectKeelViews } from '@globalart/keel/renderer';
import { readDump } from '@globalart/keel/testing';

const VIEWS = resolve(__dirname, '../views.json');
const DUMP = resolve(__dirname, '../../../../contracts/dump');
const LOGO = `/brand/${DNS_ID}/logo.svg`;

describe('@globalart/keel-dns', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('carries the dns id and registers no custom view, as views.json says', () => {
    const listed = JSON.parse(readFileSync(VIEWS, 'utf8')) as string[];
    expect(listed).toEqual([]);
    expect(DNS_VIEWS.map((view) => view.func)).toEqual(listed);
    TestBed.configureTestingModule({ providers: [provideKeelDns()] });
    const views = TestBed.runInInjectionContext(injectKeelViews);
    expect(views).toEqual([]);
    expect(DNS_ID).toBe('dns');
  });

  it('carries the brand and the group icon the server answers in /boot', () => {
    const dump = readDump(DUMP, 'user');
    const product = dump.boot.products.find((entry) => entry.id === DNS_ID);
    expect(product?.brand.logo).toBe(LOGO);
    const name = product?.brand.name ?? '';
    expect(lookup(dump.catalogues['en']?.messages ?? {}, name)).toBe(DNS_BRAND);
    const group = dump.boot.menu.find((entry) => entry.product === DNS_ID);
    expect(group?.id).toBe(DNS_ID);
    expect(group?.icon).toBe(DNS_ICON);
    expect(isIconName(DNS_ICON)).toBe(true);
  });
});
