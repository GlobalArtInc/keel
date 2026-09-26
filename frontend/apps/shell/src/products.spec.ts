import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createEnvironmentInjector, EnvironmentInjector } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { KEEL_VIEWS } from '@globalart/keel/renderer';
import { SHELL_PRODUCTS } from './products';

const DEMO_VIEWS = resolve(__dirname, '../../../products/demo/views.json');

describe('the products of the shell application', () => {
  it('lists demo, dns and panel, the dns chunk resolving no custom view and the panel chunk the file manager and the configuration editor', async () => {
    expect(SHELL_PRODUCTS.map((product) => product.id)).toEqual(['demo', 'dns', 'panel']);
    const resolved: string[][] = [];
    for (const product of SHELL_PRODUCTS.slice(1)) {
      const providers = await product.load();
      const injector = createEnvironmentInjector([providers], TestBed.inject(EnvironmentInjector));
      resolved.push(
        injector
          .get(KEEL_VIEWS)
          .flat()
          .map((view) => view.func),
      );
    }
    expect(resolved).toEqual([[], ['hosting.file-manager', 'hosting.site.config-editor']]);
  });

  it('lists demo, whose chunk resolves the views its views.json names', async () => {
    const providers = await SHELL_PRODUCTS[0].load();
    const injector = createEnvironmentInjector([providers], TestBed.inject(EnvironmentInjector));
    const views = injector.get(KEEL_VIEWS).flat();
    const listed = JSON.parse(readFileSync(DEMO_VIEWS, 'utf8')) as string[];
    expect(views.map((view) => view.func)).toEqual(listed);
  });
});
