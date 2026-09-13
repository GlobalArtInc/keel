import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import {
  CLOUD_PAGE_SIZE,
  MAX_SIZE,
  MIN_SIZE,
  provideKeelDemo,
  sizeOf,
  sized,
} from '@globalart/keel-demo';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../contracts/fixtures');
const SNAPSHOTS = resolve(__dirname, '../../../../products/demo/tests/snapshots');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function snapshot(func: string, level: string): Descriptor {
  const text = readFileSync(join(SNAPSHOTS, `${func}@${level}.snap`), 'utf8');
  const body = text.slice(text.indexOf('\n---\n', text.indexOf('---\n') + 4) + 5);
  return JSON.parse(body) as Descriptor;
}

const TAG_LIST = snapshot('note.tag.list', 'user');
const BOOT = load<Boot>('boot.json');

const MESSAGES = {
  note: { tag: { list: { title: 'Tags' }, empty: 'No tags yet' } },
  keel: {
    action: { retry: 'Retry' },
    error: { forbidden: 'You are not allowed to do this', request: '{message} (request {id})' },
  },
};

function page(rows: unknown[], total: number): Envelope {
  return { ok: true, data: { data: rows, total, totals: null }, meta: { requestId: 'req-page' } };
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

async function render(
  answers: Record<string, Envelope | Envelope[]>,
  params?: Record<string, unknown>,
): Promise<ComponentFixture<KeelScreenComponent>> {
  const client = new KeelMockClient({
    boot: BOOT,
    descriptors: [TAG_LIST],
    catalogue: { lang: 'en', messages: MESSAGES },
    answers,
  });
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [
      providePhoenixIcons(),
      provideKeelDemo(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', 'note.tag.list');
  if (params !== undefined) {
    fixture.componentRef.setInput('params', params);
  }
  fixture.detectChanges();
  await settle(fixture);
  await settle(fixture);
  return fixture;
}

function tags(element: HTMLElement): HTMLButtonElement[] {
  return [...element.querySelectorAll<HTMLButtonElement>('demo-note-tag-cloud button.tag')];
}

describe('the tag cloud of the demo', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('the tag cloud lists every tag of note.tag.list sized by count and opens the list searched by the tag', async () => {
    const fixture = await render({
      'note.tag.list': [
        page(
          [
            { tag: 'work', count: 5 },
            { tag: 'home', count: 2 },
          ],
          3,
        ),
        page([{ tag: 'urgent', count: 1 }], 3),
      ],
    });
    const element: HTMLElement = fixture.nativeElement;
    const client = TestBed.inject(KeelClient) as KeelMockClient;
    expect(element.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('phx-data-table')).toBeNull();
    const cloud = element.querySelector('demo-note-tag-cloud');
    expect(cloud?.getAttribute('data-cloud')).toBe('note.tag.list');
    expect(cloud?.getAttribute('data-phase')).toBe('loaded');
    expect(element.querySelector('phx-page-header .title__text')?.textContent).toBe('Tags');
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['note.tag.list', { page: 1, pageSize: CLOUD_PAGE_SIZE }],
      ['note.tag.list', { page: 2, pageSize: CLOUD_PAGE_SIZE }],
    ]);
    expect(tags(element).map((tag) => tag.textContent?.trim())).toEqual(['work', 'home', 'urgent']);
    expect(tags(element).map((tag) => tag.getAttribute('data-count'))).toEqual(['5', '2', '1']);
    expect(tags(element).map((tag) => tag.style.fontSize)).toEqual([
      `${MAX_SIZE}px`,
      `${sizeOf(2, 1, 5)}px`,
      `${MIN_SIZE}px`,
    ]);

    tags(element)[1].click();
    await settle(fixture);
    expect(TestBed.inject(Router).url).toBe('/f/note.list?search=home');
  });

  it('the cloud passes the screen parameters as the scope and shows the empty state without tags', async () => {
    const fixture = await render({ 'note.tag.list': page([], 0) }, { view: 'mine' });
    const element: HTMLElement = fixture.nativeElement;
    const client = TestBed.inject(KeelClient) as KeelMockClient;
    expect(client.calls.map((call) => call.params)).toEqual([
      { page: 1, pageSize: CLOUD_PAGE_SIZE, scope: { view: 'mine' } },
    ]);
    expect(tags(element)).toHaveLength(0);
    expect(element.querySelector('demo-note-tag-cloud phx-empty .title')?.textContent).toBe(
      'No tags yet',
    );
  });

  it('a refused page shows phx-error with the request id and retries', async () => {
    const fixture = await render({
      'note.tag.list': [
        {
          ok: false,
          error: { code: 'forbidden', key: 'keel.error.forbidden', requestId: 'req-denied' },
        },
        page([{ tag: 'work', count: 1 }], 1),
      ],
    });
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('demo-note-tag-cloud')?.getAttribute('data-phase')).toBe('failed');
    expect(element.querySelector('phx-error .text')?.textContent?.trim()).toBe(
      'You are not allowed to do this (request req-denied)',
    );
    element.querySelector<HTMLButtonElement>('phx-error button')?.click();
    await settle(fixture);
    expect(element.querySelector('demo-note-tag-cloud')?.getAttribute('data-phase')).toBe('loaded');
    expect(tags(element).map((tag) => tag.style.fontSize)).toEqual([
      `${Math.round((MIN_SIZE + MAX_SIZE) / 2)}px`,
    ]);
  });

  it('sizeOf scales linearly between the smallest and the largest count', () => {
    expect(sizeOf(1, 1, 5)).toBe(MIN_SIZE);
    expect(sizeOf(5, 1, 5)).toBe(MAX_SIZE);
    expect(sizeOf(3, 1, 5)).toBe(Math.round((MIN_SIZE + MAX_SIZE) / 2));
    expect(sizeOf(7, 7, 7)).toBe(Math.round((MIN_SIZE + MAX_SIZE) / 2));
    expect(sized([])).toEqual([]);
    expect(
      sized([
        { tag: 'a', count: 4 },
        { tag: 'b', count: 1 },
      ]),
    ).toEqual([
      { tag: 'a', count: 4, size: MAX_SIZE },
      { tag: 'b', count: 1, size: MIN_SIZE },
    ]);
  });
});
