import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { Boot } from '@globalart/keel/meta';
import { KEEL_BOOT } from '@globalart/keel/testing';

const CONTRACTS = resolve(__dirname, '../../../../../contracts');

function load(path: string): Boot {
  return JSON.parse(readFileSync(join(CONTRACTS, path), 'utf8')) as Boot;
}

const NARROWED = ['products', 'brand', 'menu', 'features'];

function untouched(boot: Boot): Record<string, unknown> {
  return Object.fromEntries(Object.entries(boot).filter(([key]) => !NARROWED.includes(key)));
}

describe('KEEL_BOOT', () => {
  it('carries the three committed boot payloads, the dump narrowed to demo as the one product', () => {
    expect(KEEL_BOOT.none).toEqual(load('fixtures/boot-none.json'));
    expect(KEEL_BOOT.two).toEqual(load('fixtures/boot.json'));
    const dump = load('dump/admin/boot.json');
    const demo = dump.products.filter((product) => product.id === 'demo');
    expect(KEEL_BOOT.one.products).toEqual(demo);
    expect(KEEL_BOOT.one.brand).toEqual({ ...demo[0].brand, logo: '/brand/logo.svg' });
    expect(KEEL_BOOT.one.menu).toEqual(dump.menu.filter((group) => group.product !== 'dns'));
    expect(KEEL_BOOT.one.features).toEqual(
      dump.features.filter((feature) => !feature.startsWith('dns.')),
    );
    expect(untouched(KEEL_BOOT.one)).toEqual(untouched(dump));
  });

  it('covers no product, one product and two products', () => {
    expect(KEEL_BOOT.none.products).toEqual([]);
    expect(KEEL_BOOT.one.products.map((product) => product.id)).toEqual(['demo']);
    expect(KEEL_BOOT.two.products.map((product) => product.id)).toEqual(['dns', 'panel']);
    expect(KEEL_BOOT.one.brand.name).toBe(KEEL_BOOT.one.products[0].brand.name);
    expect(KEEL_BOOT.one.brand.logo).toBe('/brand/logo.svg');
    expect(KEEL_BOOT.two.brand.name).toBe('keel.brand');
    expect(KEEL_BOOT.none.brand.name).toBe('keel.brand');
  });
});
