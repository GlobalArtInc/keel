import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { DNS_ID, DNS_VIEWS, provideKeelDns } from '@globalart/keel-dns';
import { injectKeelViews } from '@globalart/keel/renderer';

const VIEWS = resolve(__dirname, '../views.json');

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
});
