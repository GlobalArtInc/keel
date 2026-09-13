import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { DEMO_ID, DEMO_VIEWS, NoteTagCloudComponent, provideKeelDemo } from '@globalart/keel-demo';
import { injectKeelViews } from '@globalart/keel/renderer';

const VIEWS = resolve(__dirname, '../views.json');

describe('@globalart/keel-demo', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('views.json names exactly what the library registers', () => {
    const listed = JSON.parse(readFileSync(VIEWS, 'utf8')) as string[];
    expect(listed).toEqual(DEMO_VIEWS.map((view) => view.func));
    TestBed.configureTestingModule({ providers: [provideKeelDemo()] });
    const views = TestBed.runInInjectionContext(injectKeelViews);
    expect(views.map((view) => view.func)).toEqual(listed);
    expect(views.map((view) => view.component)).toEqual([NoteTagCloudComponent]);
    expect(DEMO_ID).toBe('demo');
  });
});
