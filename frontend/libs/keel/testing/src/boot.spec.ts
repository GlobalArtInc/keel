import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { Boot } from '@globalart/keel/meta';
import { KEEL_BOOT } from '@globalart/keel/testing';

const CONTRACTS = resolve(__dirname, '../../../../../contracts');

function load(path: string): Boot {
  return JSON.parse(readFileSync(join(CONTRACTS, path), 'utf8')) as Boot;
}

describe('KEEL_BOOT', () => {
  it('carries the three committed boot payloads', () => {
    expect(KEEL_BOOT.none).toEqual(load('fixtures/boot-none.json'));
    expect(KEEL_BOOT.one).toEqual(load('dump/admin/boot.json'));
    expect(KEEL_BOOT.two).toEqual(load('fixtures/boot.json'));
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
