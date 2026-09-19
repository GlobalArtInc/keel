import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { ApplicationRef } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import {
  KEEL_EVENT_SOURCE,
  KeelBoot,
  KeelClient,
  KeelEvents,
  KeelI18n,
  TASK_CANCEL,
  TASK_GET,
  type DownloadResult,
  type TaskDetail,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, TaskProgress, TaskRef } from '@globalart/keel/meta';
import {
  KeelActionRunner,
  KeelTasks,
  TASK_POLL_MS,
  type ActionRequest,
} from '@globalart/keel/renderer';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxModalService, PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const TASK = '01J9Q3W6B1R8N5X2K4T7M0V9T1';
const DOWNLOAD = 'task.download';

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const BOOT = load<Boot>('boot.json');
const LIST = load<Descriptor>('descriptor-dns-zone-list.json');

const MESSAGES = {
  keel: {
    error: { request: '{message} (request {id})', unavailable: 'Temporarily unavailable' },
    shell: { tasks: { details: 'Details', failed: 'The task {function} failed' } },
  },
  dns: {
    zone: {
      action: { export: 'Export zones' },
      export: { step: 'Zone {name}' },
      error: { export_failed: 'The zone {name} could not be exported' },
    },
  },
  phx: { cancel: 'Cancel' },
};

const EXPORT: ActionRequest = {
  descriptor: LIST,
  action: { id: 'export', role: 'button', function: 'dns.zone.export' },
};

function ok<T>(data: T, requestId = 'req-task'): Envelope<T> {
  return { ok: true, data, meta: { requestId } };
}

function ref(): TaskRef {
  return { task: { id: TASK } };
}

function frame(patch: Partial<TaskProgress>): TaskProgress {
  return { id: TASK, function: 'dns.zone.export', state: 'running', done: 0, ...patch };
}

function detail(patch: Partial<TaskDetail>): TaskDetail {
  return {
    id: TASK,
    function: 'dns.zone.export',
    class: 'export',
    state: 'running',
    progress: '0',
    done: 0,
    resumable: false,
    requestId: 'req-job',
    createdAt: '2026-09-20T10:00:00Z',
    accountId: 'a',
    principalId: 'p',
    params: {},
    logTail: [],
    ...patch,
  };
}

function client(): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: [LIST],
    catalogue: { lang: 'en', messages: MESSAGES },
    answers: { 'dns.zone.export': ok(ref()) },
  });
}

async function boot(mock: KeelMockClient): Promise<QuietEventSource> {
  TestBed.configureTestingModule({
    providers: [
      providePhoenixIcons(),
      { provide: KeelClient, useValue: mock },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  TestBed.inject(KeelEvents).subscribe();
  const stream = QuietEventSource.last();
  if (stream === null) {
    throw new Error('the event stream did not open');
  }
  stream.open();
  return stream;
}

function tick(): void {
  TestBed.inject(ApplicationRef).tick();
}

async function settle(): Promise<void> {
  for (let round = 0; round < 5; round += 1) {
    await Promise.resolve();
  }
  tick();
}

function modal(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.cdk-overlay-container keel-progress');
}

function text(selector: string): string {
  return modal()?.querySelector(selector)?.textContent?.trim() ?? '';
}

describe('KeelTasks', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(PhxModalService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
    jest.useRealTimers();
  });

  it('a TaskRef answer opens the progress modal, frames drive value and status, cancel posts task.cancel and done hands back to the opener', async () => {
    const mock = client();
    mock.answer(TASK_CANCEL, ok({ id: TASK, state: 'cancelled' }));
    mock.answer(TASK_GET, ok(detail({ state: 'done', done: 3, total: 3, result: { zones: 3 } })));
    const stream = await boot(mock);
    const runner = TestBed.inject(KeelActionRunner);

    const cancelled = runner.run(EXPORT);
    await settle();
    expect(modal()).not.toBeNull();
    expect(text('.title')).toBe('Export zones');

    stream.emit('task.progress', frame({ done: 1, total: 4, message: 'dns.zone.export.step' }));
    tick();
    expect(text('.percent')).toBe('25%');
    expect(text('.status')).toBe('Zone {name}');

    const cancel = modal()?.querySelector<HTMLButtonElement>('.actions button');
    cancel?.click();
    await settle();
    expect(await cancelled).toBeNull();
    expect(mock.callsOf(TASK_CANCEL).map((call) => call.params)).toEqual([{ id: TASK }]);
    expect(modal()).toBeNull();

    const done = runner.run(EXPORT);
    await settle();
    stream.emit('task.progress', frame({ state: 'done', done: 3, total: 3 }));
    await settle();
    expect(await done).toEqual(ok({ zones: 3 }, 'req-job'));
    expect(mock.callsOf(TASK_GET).map((call) => call.params)).toEqual([{ id: TASK }]);
    expect(modal()).toBeNull();
  });

  it('a result with a download member fetches the download route and hands the blob to the browser', async () => {
    const mock = client();
    const link = { function: DOWNLOAD, params: { id: TASK } };
    mock.answer(TASK_GET, ok(detail({ state: 'done', result: { exported: 2, download: link } })));
    const file: DownloadResult = {
      ok: true,
      file: { blob: new Blob(['a,b\n']), filename: 'zones.csv' },
    };
    mock.file(DOWNLOAD, file);
    const created: Blob[] = [];
    const revoked: string[] = [];
    const clicks: string[] = [];
    URL.createObjectURL = jest.fn((blob: Blob) => {
      created.push(blob);
      return 'blob:zones';
    });
    URL.revokeObjectURL = jest.fn((url: string) => void revoked.push(url));
    const click = jest.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicks.push(`${this.getAttribute('href')} as ${this.download}`);
    });
    const stream = await boot(mock);

    const outcome = TestBed.inject(KeelTasks).follow(ref(), { title: 'Export zones' });
    await settle();
    stream.emit('task.progress', frame({ state: 'done', done: 2, total: 2 }));
    await settle();

    expect(await outcome).toEqual(ok({ exported: 2, download: link }, 'req-job'));
    expect(mock.downloads).toEqual([{ func: DOWNLOAD, params: { id: TASK } }]);
    expect(created).toEqual([file.ok ? file.file.blob : null]);
    expect(clicks).toEqual(['blob:zones as zones.csv']);
    click.mockRestore();
  });

  it('a failed frame shows the error and the request id', async () => {
    const mock = client();
    mock.answer(
      TASK_GET,
      ok(
        detail({
          state: 'failed',
          error: {
            code: 'internal',
            key: 'dns.zone.error.export_failed',
            params: { name: 'a.org' },
            requestId: 'req-9',
          },
        }),
      ),
    );
    const stream = await boot(mock);

    const outcome = TestBed.inject(KeelActionRunner).run(EXPORT);
    await settle();
    stream.emit('task.progress', frame({ state: 'failed', done: 1, total: 4 }));
    await settle();

    const envelope = await outcome;
    expect(envelope?.ok).toBe(false);
    const toasts = TestBed.inject(PhxToastService).toasts();
    expect(toasts).toHaveLength(1);
    expect(toasts[0].status).toBe('danger');
    expect(toasts[0].title).toBe('Export zones');
    expect(toasts[0].text).toBe('The zone a.org could not be exported (request req-9)');
    expect(modal()).toBeNull();
  });

  it('a modal without frames polls task.get', async () => {
    jest.useFakeTimers();
    const mock = client();
    mock.answer(TASK_GET, [
      ok(detail({ state: 'running', done: 1, total: 2, message: 'dns.zone.export.step' })),
      ok(detail({ state: 'done', done: 2, total: 2, result: { zones: 2 } })),
    ]);
    await boot(mock);

    const outcome = TestBed.inject(KeelActionRunner).run(EXPORT);
    await settle();
    expect(mock.callsOf(TASK_GET)).toHaveLength(0);

    jest.advanceTimersByTime(TASK_POLL_MS);
    await settle();
    expect(mock.callsOf(TASK_GET)).toHaveLength(1);
    expect(text('.percent')).toBe('50%');
    expect(text('.status')).toBe('Zone {name}');

    jest.advanceTimersByTime(TASK_POLL_MS);
    await settle();
    expect(mock.callsOf(TASK_GET)).toHaveLength(3);
    expect(await outcome).toEqual(ok({ zones: 2 }, 'req-job'));
  });
});
