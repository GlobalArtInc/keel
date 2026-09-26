import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createEnvironmentInjector, EnvironmentInjector } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { KEEL_VIEWS } from '@globalart/keel/renderer';
import { SHELL_PRODUCTS } from './products';

const DEMO_VIEWS = resolve(__dirname, '../../../products/demo/views.json');

describe('the products of the shell application', () => {
  it('lists demo, dns and panel, and the dns and panel chunks resolve no custom view', async () => {
    expect(SHELL_PRODUCTS.map((product) => product.id)).toEqual(['demo', 'dns', 'panel']);
    for (const product of SHELL_PRODUCTS.slice(1)) {
      const providers = await product.load();
      const injector = createEnvironmentInjector([providers], TestBed.inject(EnvironmentInjector));
      expect(injector.get(KEEL_VIEWS).flat()).toEqual([]);
    }
  });

  it('lists demo, whose chunk resolves the views its views.json names', async () => {
    const providers = await SHELL_PRODUCTS[0].load();
    const injector = createEnvironmentInjector([providers], TestBed.inject(EnvironmentInjector));
    const views = injector.get(KEEL_VIEWS).flat();
    const listed = JSON.parse(readFileSync(DEMO_VIEWS, 'utf8')) as string[];
    expect(views.map((view) => view.func)).toEqual(listed);
  });
});
