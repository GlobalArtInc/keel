import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import {
  PANEL_BRAND,
  PANEL_ICON,
  PANEL_ID,
  PANEL_VIEWS,
  provideKeelPanel,
} from '@globalart/keel-panel';
import { isIconName } from '@globalart/keel/forms';
import { lookup } from '@globalart/keel/meta';
import { injectKeelViews } from '@globalart/keel/renderer';
import { readDump } from '@globalart/keel/testing';

const VIEWS = resolve(__dirname, '../views.json');
const DUMP = resolve(__dirname, '../../../../contracts/dump');
const LOGO = `/brand/${PANEL_ID}/logo.svg`;

describe('@globalart/keel-panel', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('carries the panel id and registers the file manager, the view the server registers, and no replacement of a screen', () => {
    const listed = JSON.parse(readFileSync(VIEWS, 'utf8')) as string[];
    expect(listed).toEqual([]);
    const registered = readDump(DUMP, 'user').boot.views;
    expect(registered).toEqual(['hosting.file-manager']);
    expect(PANEL_VIEWS.map((view) => view.func)).toEqual([...registered, ...listed]);
    TestBed.configureTestingModule({ providers: [provideKeelPanel()] });
    const views = TestBed.runInInjectionContext(injectKeelViews);
    expect(views.map((view) => view.func)).toEqual(['hosting.file-manager']);
    expect(PANEL_ID).toBe('panel');
  });

  it('carries the brand and the group icon the server answers in /boot', () => {
    const dump = readDump(DUMP, 'user');
    const product = dump.boot.products.find((entry) => entry.id === PANEL_ID);
    expect(product?.brand.logo).toBe(LOGO);
    const name = product?.brand.name ?? '';
    expect(lookup(dump.catalogues['en']?.messages ?? {}, name)).toBe(PANEL_BRAND);
    const group = dump.boot.menu.find((entry) => entry.product === PANEL_ID);
    expect(group?.id).toBe(PANEL_ID);
    expect(group?.icon).toBe(PANEL_ICON);
    expect(isIconName(PANEL_ICON)).toBe(true);
  });
});
