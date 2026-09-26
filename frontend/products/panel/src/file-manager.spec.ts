import { resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { provideKeelPanel } from '@globalart/keel-panel';
import type { Envelope } from '@globalart/keel/meta';
import { KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient, fixturesOf, readDump } from '@globalart/keel/testing';
import { PhxModalService, PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';
import {
  crumbsOf,
  pageOf,
  sizeOf,
  sorted,
  validMode,
  validName,
  type FileEntry,
} from './file-manager/files';

const DUMP = resolve(__dirname, '../../../../contracts/dump');
const VIEW = 'hosting.file-manager';
const USER = '01J9ZX0000000000000000FILE';

const ROOT: FileEntry[] = [
  { name: 'notes.txt', kind: 'file', size: 2048, mode: '0644', modified: '2026-09-01T10:00:00Z' },
  { name: 'site', kind: 'dir', size: 4096, mode: '0755', modified: '2026-09-01T10:00:00Z' },
];
const SITE: FileEntry[] = [
  { name: 'index.html', kind: 'file', size: 12, mode: '0644', modified: '2026-09-02T10:00:00Z' },
];

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-files' } };
}

function listing(entries: FileEntry[]): Envelope {
  return ok({ entries, truncated: false });
}

const USERS = ok({ data: [{ value: USER, label: 'hp_files' }], more: false });
const TASK = ok({ task: { id: '01J9ZX00000000000000000TSK' } });

function task(state: string, error: unknown = null): Envelope {
  return ok({
    id: '01J9ZX00000000000000000TSK',
    function: 'hosting.file.apply',
    class: 'hosting.file',
    state,
    progress: '',
    done: 0,
    resumable: false,
    createdAt: '2026-09-02T10:00:00Z',
    accountId: '01J9ZX0000000000000000ACNT',
    principalId: '01J9ZX0000000000000000PRCP',
    params: {},
    error,
    logTail: [],
  });
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  for (let round = 0; round < 3; round++) {
    await fixture.whenStable();
    await new Promise((done) => setTimeout(done, 0));
    fixture.detectChanges();
  }
}

async function render(
  answers: Record<string, Envelope | Envelope[]>,
  params: Record<string, unknown> = { user: USER },
): Promise<{ fixture: ComponentFixture<KeelScreenComponent>; client: KeelMockClient }> {
  const client = new KeelMockClient({ ...fixturesOf(readDump(DUMP, 'user')), answers });
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [
      providePhoenixIcons(),
      provideKeelPanel(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', VIEW);
  fixture.componentRef.setInput('params', params);
  fixture.detectChanges();
  await settle(fixture);
  return { fixture, client };
}

function names(element: HTMLElement): string[] {
  return [...element.querySelectorAll('[data-entry]')].map(
    (entry) => entry.getAttribute('data-entry') ?? '',
  );
}

function overlay(): HTMLElement {
  const container = document.querySelector<HTMLElement>('.cdk-overlay-container');
  if (container === null) {
    throw new Error('no overlay container');
  }
  return container;
}

function click(target: Element | null): void {
  if (!(target instanceof HTMLElement)) {
    throw new Error('nothing to click');
  }
  target.click();
}

function type(input: Element | null, value: string): void {
  if (!(input instanceof HTMLInputElement)) {
    throw new Error('no input to type into');
  }
  input.value = value;
  input.dispatchEvent(new Event('input'));
}

function submit(form: Element | null): void {
  if (!(form instanceof HTMLFormElement)) {
    throw new Error('no form to submit');
  }
  form.dispatchEvent(new Event('submit'));
}

describe('the file manager of panel', () => {
  afterEach(() => {
    TestBed.inject(PhxModalService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('lists the home of the hosting user folders first and enters a folder through its row', async () => {
    const { fixture, client } = await render({
      'hosting.user.options': USERS,
      'hosting.file.list': [listing(ROOT), listing(SITE)],
    });
    const element: HTMLElement = fixture.nativeElement;
    const view = element.querySelector('panel-file-manager');
    expect(view?.getAttribute('data-phase')).toBe('loaded');
    expect(element.querySelector('phx-page-header .title__text')?.textContent).toBe('Files');
    expect(names(element)).toEqual(['site', 'notes.txt']);
    expect(client.callsOf('hosting.file.list').map((call) => call.params)).toEqual([
      { user: USER, path: '' },
    ]);

    click(element.querySelector('[data-entry="site"]'));
    await settle(fixture);
    expect(client.callsOf('hosting.file.list').map((call) => call.params)).toEqual([
      { user: USER, path: '' },
      { user: USER, path: 'site' },
    ]);
    expect(view?.getAttribute('data-path')).toBe('site');
    expect(names(element)).toEqual(['index.html']);
  });

  it('makes a folder through the prompt and waits for its task before it lists again', async () => {
    const { fixture, client } = await render({
      'hosting.user.options': USERS,
      'hosting.file.list': [listing(ROOT), listing([...ROOT, { ...ROOT[1], name: 'logs' }])],
      'hosting.file.mkdir': TASK,
      'task.get': [task('running'), task('done')],
    });
    const element: HTMLElement = fixture.nativeElement;
    click(element.querySelector('button.mkdir'));
    await settle(fixture);
    const prompt = overlay().querySelector('panel-file-prompt');
    expect(overlay().querySelector('phx-modal .title')?.textContent).toBe('New folder');
    type(prompt?.querySelector('phx-input input') ?? null, 'a/b');
    submit(prompt?.querySelector('form') ?? null);
    await settle(fixture);
    expect(prompt?.textContent).toContain('A name is not empty');
    expect(client.callsOf('hosting.file.mkdir')).toHaveLength(0);
    type(prompt?.querySelector('phx-input input') ?? null, 'logs');
    submit(prompt?.querySelector('form') ?? null);
    await new Promise((done) => setTimeout(done, 400));
    await settle(fixture);
    expect(client.callsOf('hosting.file.mkdir').map((call) => call.params)).toEqual([
      { user: USER, path: 'logs' },
    ]);
    expect(client.callsOf('task.get')).toHaveLength(2);
    expect(client.callsOf('hosting.file.list')).toHaveLength(2);
    expect(names(element)).toEqual(['logs', 'site', 'notes.txt']);
  });

  it('shows the error of a failed task and uploads a file as an octet stream into the folder', async () => {
    const { fixture, client } = await render({
      'hosting.user.options': USERS,
      'hosting.file.list': listing(ROOT),
      'hosting.file.upload': TASK,
      'task.get': task('failed', {
        code: 'forbidden',
        key: 'hosting.file.error.outside',
        params: { path: 'site/../../x' },
      }),
    });
    const element: HTMLElement = fixture.nativeElement;
    const picker = element.querySelector<HTMLInputElement>('input[data-picker="upload"]');
    if (picker === null) {
      throw new Error('no file picker');
    }
    const file = new File(['<p>hello</p>'], 'index.html', { type: 'text/html' });
    Object.defineProperty(picker, 'files', { value: [file], configurable: true });
    picker.dispatchEvent(new Event('change'));
    await settle(fixture);
    const upload = client.uploads[0];
    expect(upload?.func).toBe('hosting.file.upload');
    expect(upload?.params).toEqual({ user: USER, path: 'index.html' });
    expect(upload?.files['content']?.[0]?.type).toBe('application/octet-stream');
    expect(upload?.files['content']?.[0]?.name).toBe('index.html');
    const toasts = TestBed.inject(PhxToastService).toasts();
    expect(toasts.map((toast) => toast.text)).toEqual([
      'site/../../x leads outside the home directory of the hosting user',
    ]);
  });

  it('asks for a hosting user before it lists anything', async () => {
    const { fixture, client } = await render({ 'hosting.user.options': USERS }, {});
    const element: HTMLElement = fixture.nativeElement;
    expect(client.callsOf('hosting.file.list')).toHaveLength(0);
    expect(element.querySelector('panel-file-manager phx-empty')?.textContent).toContain(
      'Choose a hosting user',
    );
  });

  it('pages the entries folders first, filters them by name and names the path in crumbs', () => {
    expect(sorted(ROOT).map((entry) => entry.name)).toEqual(['site', 'notes.txt']);
    expect(pageOf(ROOT, { offset: 0, limit: 1, search: '' })).toEqual({
      data: [ROOT[1]],
      total: 2,
    });
    expect(pageOf(ROOT, { offset: 0, limit: 10, search: 'NOTE' }).data).toEqual([ROOT[0]]);
    expect(crumbsOf('site/logs', 'Home')).toEqual([
      { label: 'Home', path: '' },
      { label: 'site', path: 'site' },
      { label: 'logs', path: 'site/logs' },
    ]);
    expect(['a', 'a b', '.x'].every(validName)).toBe(true);
    expect(['', '.', '..', 'a/b'].some(validName)).toBe(false);
    expect(['644', '0755'].every(validMode)).toBe(true);
    expect(['8', '64', '1777', '07777', 'rw'].some(validMode)).toBe(false);
    expect(sizeOf(12)).toBe('12 B');
    expect(sizeOf(2048)).toBe('2.0 KiB');
    expect(sizeOf(5 * 1024 * 1024)).toBe('5.0 MiB');
  });
});
