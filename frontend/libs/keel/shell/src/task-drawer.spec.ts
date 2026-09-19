import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  KEEL_EVENT_SOURCE,
  KeelClient,
  KeelEvents,
  TASK_CANCEL,
  TASK_GET,
  TASK_LIST,
  type TaskDetail,
  type TaskRow,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, Messages, TaskProgress } from '@globalart/keel/meta';
import { KeelTaskWatch, keelProviders } from '@globalart/keel/shell';
import { KeelMockClient, QuietEventSource, pageEnvelope } from '@globalart/keel/testing';
import { PhxDrawerService, PhxToastService } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const RUNNING = '01J9Q3W6B1R8N5X2K4T7M0V9T1';
const QUEUED = '01J9Q3W6B1R8N5X2K4T7M0V9T2';
const DONE = '01J9Q3W6B1R8N5X2K4T7M0V9T3';

const EN: Messages = {
  menu: { system: 'System' },
  keel: {
    brand: 'Keel',
    level: { owner: 'Owner' },
    account: { list: { title: 'Accounts' } },
    error: { request: '{message} (request {id})' },
    shell: {
      rail: { notifications: 'Notifications', tasks: 'Tasks', help: 'Help' },
      tasks: {
        title: 'Tasks',
        empty: 'No tasks have run yet',
        log: 'Log',
        id: 'Task',
        request: 'Request',
      },
      user: { language: 'Language', theme: 'Theme' },
    },
  },
  task: {
    column: { function: 'Function', state: 'State', progress: 'Progress', createdAt: 'Created' },
    state: { queued: 'Queued', running: 'Running', done: 'Done', cancelled: 'Cancelled' },
  },
  note: { export: { title: 'Export notes' } },
  phx: { back: 'Back', cancel: 'Cancel', close: 'Close', retry: 'Retry' },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function descriptor(name: string, kind: Descriptor['kind']): Descriptor {
  return {
    keel: 1,
    name,
    module: 'task',
    kind,
    mode: kind === 'action' ? 'write' : 'read',
    access: { level: 'user' },
    params: {},
    result: {},
  };
}

function row(id: string, state: TaskRow['state'], done: number, total: number | null): TaskRow {
  return {
    id,
    function: 'note.export',
    class: 'export',
    state,
    progress: total === null ? String(done) : `${done}/${total}`,
    done,
    total,
    resumable: true,
    requestId: `req-${id.slice(-2)}`,
    createdAt: '2026-09-20T10:00:00Z',
    accountId: 'a',
    principalId: 'p',
  };
}

function detail(base: TaskRow, logTail: string[]): TaskDetail {
  return { ...base, params: {}, logTail };
}

function ok<T>(data: T): Envelope<T> {
  return { ok: true, data, meta: { requestId: 'req-drawer' } };
}

function frame(patch: Partial<TaskProgress>): TaskProgress {
  return { id: RUNNING, function: 'note.export', state: 'running', done: 0, ...patch };
}

interface Rendered {
  harness: RouterTestingHarness;
  client: KeelMockClient;
  element: HTMLElement;
  stream: QuietEventSource;
}

async function render(client: KeelMockClient): Promise<Rendered> {
  TestBed.configureTestingModule({
    providers: [
      ...keelProviders(),
      { provide: KeelClient, useValue: client },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/f/keel.account.list');
  TestBed.inject(KeelEvents).subscribe();
  await settle(harness);
  const stream = QuietEventSource.last();
  if (stream === null) {
    throw new Error('the event stream did not open');
  }
  stream.open();
  return { harness, client, element: harness.fixture.nativeElement, stream };
}

async function settle(harness: RouterTestingHarness): Promise<void> {
  await harness.fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  harness.detectChanges();
}

function drawer(element: HTMLElement): HTMLElement | null {
  return element.ownerDocument.querySelector<HTMLElement>('keel-task-drawer');
}

function badge(element: HTMLElement): string | null {
  return (
    element.querySelector('phx-rail-item[data-rail="tasks"] .badge')?.textContent?.trim() ?? null
  );
}

function client(): KeelMockClient {
  return new KeelMockClient({
    boot: load<Boot>('boot-none.json'),
    descriptors: [
      descriptor(TASK_LIST, 'list'),
      descriptor(TASK_GET, 'get'),
      descriptor(TASK_CANCEL, 'action'),
    ],
    catalogue: { lang: 'en', messages: EN },
    answers: {
      [TASK_LIST]: pageEnvelope([
        row(RUNNING, 'running', 1, 4),
        row(QUEUED, 'queued', 0, null),
        row(DONE, 'done', 3, 3),
      ]),
    },
  });
}

describe('the task drawer', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(PhxDrawerService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('the rail badge counts queued and running jobs and the drawer lists them', async () => {
    const { harness, element, stream, client: mock } = await render(client());
    expect(badge(element)).toBe('2');
    expect(mock.callsOf(TASK_LIST)).toHaveLength(1);

    stream.emit('task.progress', frame({ id: QUEUED, state: 'done', done: 1, total: 1 }));
    await settle(harness);
    expect(badge(element)).toBe('1');
    expect(TestBed.inject(KeelTaskWatch).count()).toBe(1);

    mock.answer(
      TASK_LIST,
      pageEnvelope([
        row(RUNNING, 'running', 2, 4),
        row(QUEUED, 'done', 1, 1),
        row(DONE, 'done', 3, 3),
      ]),
    );
    element.querySelector<HTMLElement>('phx-rail-item[data-rail="tasks"] button')?.click();
    await settle(harness);
    const open = drawer(element);
    expect(open).not.toBeNull();
    expect(mock.callsOf(TASK_LIST)).toHaveLength(2);
    const jobs = [...(open?.querySelectorAll('button.job') ?? [])];
    expect(jobs.map((job) => job.getAttribute('data-job'))).toEqual([RUNNING, QUEUED, DONE]);
    expect(jobs.map((job) => job.querySelector('phx-badge')?.textContent?.trim())).toEqual([
      'Running',
      'Done',
      'Done',
    ]);
    expect(jobs.map((job) => job.querySelector('.job__progress')?.textContent)).toEqual([
      '2/4',
      '1/1',
      '3/3',
    ]);
    expect(jobs[0].querySelector('.job__function')?.textContent).toBe('Export notes');
  });

  it("a job's detail shows the prop list, the progress and the log tail refreshed by its frames and offers cancel while it runs", async () => {
    const mock = client();
    const running = row(RUNNING, 'running', 1, 4);
    mock.answer(TASK_GET, [
      ok(detail(running, ['exported Alpha'])),
      ok(detail({ ...running, done: 2, progress: '2/4' }, ['exported Alpha', 'exported Beta'])),
      ok(
        detail({ ...running, state: 'cancelled', progress: '2/4' }, [
          'exported Alpha',
          'exported Beta',
        ]),
      ),
    ]);
    mock.answer(TASK_CANCEL, ok({ id: RUNNING, state: 'cancelled' }));
    const { harness, element, stream } = await render(mock);
    element.querySelector<HTMLElement>('phx-rail-item[data-rail="tasks"] button')?.click();
    await settle(harness);
    const open = drawer(element);
    open?.querySelector<HTMLElement>(`button.job[data-job="${RUNNING}"]`)?.click();
    await settle(harness);

    expect(open?.getAttribute('data-job')).toBe(RUNNING);
    const props = [...(open?.querySelectorAll('phx-prop-list phx-prop') ?? [])].map((prop) =>
      prop.textContent?.replace(/\s+/g, ' ').trim(),
    );
    expect(props).toEqual([
      `Task ${RUNNING}`,
      'Function Export notes',
      'State Running',
      'Progress 1/4',
      expect.stringMatching(/^Created /),
      'Request req-T1',
    ]);
    expect(open?.querySelector('phx-progress-bar')?.getAttribute('aria-valuenow')).toBe('25');
    expect(open?.querySelector<HTMLTextAreaElement>('phx-code-field textarea')?.value).toBe(
      'exported Alpha',
    );
    expect(open?.querySelector('button.cancel')).not.toBeNull();

    stream.emit('task.progress', frame({ done: 2, total: 4 }));
    await settle(harness);
    expect(open?.querySelector('phx-progress-bar')?.getAttribute('aria-valuenow')).toBe('50');
    expect(open?.querySelector<HTMLTextAreaElement>('phx-code-field textarea')?.value).toBe(
      'exported Alpha\nexported Beta',
    );

    open?.querySelector<HTMLElement>('button.cancel')?.click();
    await settle(harness);
    expect(mock.callsOf(TASK_CANCEL).map((call) => call.params)).toEqual([{ id: RUNNING }]);
    stream.emit('task.progress', frame({ state: 'cancelled', done: 2, total: 4 }));
    await settle(harness);
    expect(open?.querySelector('button.cancel')).toBeNull();
    expect(open?.querySelector('phx-prop-list')?.textContent).toContain('Cancelled');

    open?.querySelector<HTMLElement>('button.back')?.click();
    await settle(harness);
    expect(open?.getAttribute('data-job')).toBeNull();
    expect(open?.querySelectorAll('button.job')).toHaveLength(3);
  });
});
