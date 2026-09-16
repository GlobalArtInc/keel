import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import {
  KEEL_EVENTS_PATH,
  KEEL_EVENT_SOURCE,
  KeelAuth,
  KeelBoot,
  KeelClient,
  KeelDescriptors,
  KeelEvents,
  MAX_BACKOFF_MS,
  MIN_BACKOFF_MS,
  isEvent,
  isMetaInvalidate,
  isNotificationFrame,
  isTaskProgress,
  provideKeelClient,
} from '@globalart/keel/client';
import type {
  Boot,
  BootPrincipal,
  Descriptor,
  Event as KeelEvent,
  NotificationFrame,
  TaskProgress,
} from '@globalart/keel/meta';
import { QuietEventSource } from '@globalart/keel/testing';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;
const GUEST: Boot = { ...BOOT, principal: null, meta: 'm-guest' };
const EVENT = JSON.parse(readFileSync(join(FIXTURES, 'event.json'), 'utf8')) as KeelEvent;
const LIST = JSON.parse(
  readFileSync(join(FIXTURES, 'descriptor-dns-zone-list.json'), 'utf8'),
) as Descriptor;

const PROGRESS: TaskProgress = {
  id: '01J9T0000000000000000000A1',
  function: 'note.export',
  state: 'running',
  done: 40,
  total: 100,
  message: null,
};

const NOTE: NotificationFrame = {
  id: '01J9N0000000000000000000A1',
  key: 'notify.problem.raised',
  params: { key: 'note.overdue' },
  level: 'warning',
  createdAt: '2026-09-12T10:00:00.000Z',
};

const OTHER_ID = '01J8A0000000000000000000C1';
const CONNECTING = 0;
const CLOSED = 2;

interface Received {
  task: TaskProgress[];
  notification: NotificationFrame[];
  data: KeelEvent[];
  meta: unknown[];
}

function principalOf(boot: Boot): BootPrincipal {
  if (!boot.principal) {
    throw new Error('the boot fixture has no principal');
  }
  return boot.principal;
}

function last(): QuietEventSource {
  const source = QuietEventSource.last();
  if (source === null) {
    throw new Error('no event source was opened');
  }
  return source;
}

function settled(): Promise<void> {
  return new Promise((done) => setTimeout(done, 0));
}

function configure(): { http: HttpTestingController; events: KeelEvents; boot: KeelBoot } {
  TestBed.configureTestingModule({
    providers: [
      provideHttpClient(),
      provideHttpClientTesting(),
      provideKeelClient(),
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  return {
    http: TestBed.inject(HttpTestingController),
    events: TestBed.inject(KeelEvents),
    boot: TestBed.inject(KeelBoot),
  };
}

async function booted(http: HttpTestingController, boot: KeelBoot, payload = BOOT): Promise<void> {
  const loading = boot.load();
  http.expectOne({ method: 'GET', url: '/api/v1/boot' }).flush(payload, {
    headers: { ETag: `"${payload.meta}"` },
  });
  await loading;
  TestBed.flushEffects();
}

function listen(events: KeelEvents): Received {
  const received: Received = { task: [], notification: [], data: [], meta: [] };
  events.taskProgress$.subscribe((frame) => received.task.push(frame));
  events.notification$.subscribe((frame) => received.notification.push(frame));
  events.dataInvalidate$.subscribe((frame) => received.data.push(frame));
  events.metaInvalidate$.subscribe((frame) => received.meta.push(frame));
  return received;
}

describe('KeelEvents', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(KeelEvents).close();
    TestBed.inject(HttpTestingController).verify();
    jest.useRealTimers();
  });

  it('the subscription opens with credentials after a boot with a principal and closes on logout', async () => {
    const { http, events, boot } = configure();
    expect(events.state()).toEqual({ phase: 'idle', lastEventId: null, reconnects: 0 });

    events.subscribe();
    events.subscribe();
    TestBed.flushEffects();
    expect(QuietEventSource.opened).toHaveLength(0);
    expect(events.state().phase).toBe('idle');

    await booted(http, boot);
    expect(QuietEventSource.opened).toHaveLength(1);
    expect(last().url).toBe(TestBed.inject(KEEL_EVENTS_PATH));
    expect(last().url).toBe('/api/v1/events');
    expect(last().init).toEqual({ withCredentials: true });
    expect(events.state().phase).toBe('connecting');
    last().open();
    expect(events.state().phase).toBe('open');

    await booted(http, boot, { ...BOOT, meta: 'm-again' });
    expect(QuietEventSource.opened).toHaveLength(1);
    expect(last().readyState).not.toBe(CLOSED);

    const leaving = TestBed.inject(KeelAuth).logout();
    http
      .expectOne({ method: 'POST', url: '/api/v1/call/auth.logout' })
      .flush({ ok: true, data: { revoked: true }, meta: { requestId: 'req-out' } });
    await settled();
    http.expectOne({ method: 'GET', url: '/api/v1/boot' }).flush(GUEST, {
      headers: { ETag: `"${GUEST.meta}"` },
    });
    await leaving;
    TestBed.flushEffects();
    expect(last().readyState).toBe(CLOSED);
    expect(events.state()).toEqual({ phase: 'idle', lastEventId: null, reconnects: 0 });
    expect(QuietEventSource.opened).toHaveLength(1);

    await booted(http, boot, { ...BOOT, principal: { ...principalOf(BOOT), id: OTHER_ID } });
    expect(QuietEventSource.opened).toHaveLength(2);
    expect(events.state().phase).toBe('connecting');

    boot.drop();
    TestBed.flushEffects();
    expect(last().readyState).toBe(CLOSED);
    expect(events.state().phase).toBe('idle');
  });

  it('a boot answer that lands after a drop keeps the principal gone and the stream closed', async () => {
    const { http, events, boot } = configure();
    events.subscribe();
    await booted(http, boot);
    last().open();
    expect(QuietEventSource.opened).toHaveLength(1);

    last().emit('meta.invalidate', { meta: 'm-later' });
    const abandoned = http.expectOne({ method: 'GET', url: '/api/v1/boot' });
    expect(abandoned.request.headers.get('If-None-Match')).toBe(`"${BOOT.meta}"`);
    boot.drop();
    TestBed.flushEffects();
    expect(last().readyState).toBe(CLOSED);
    expect(events.state().phase).toBe('idle');

    abandoned.flush({ ...BOOT, meta: 'm-later' }, { headers: { ETag: '"m-later"' } });
    await settled();
    TestBed.flushEffects();
    expect(boot.loaded()).toBe(false);
    expect(boot.principal()).toBeNull();
    expect(QuietEventSource.opened).toHaveLength(1);
    expect(last().readyState).toBe(CLOSED);
    expect(events.state()).toEqual({ phase: 'idle', lastEventId: null, reconnects: 0 });

    await booted(http, boot);
    expect(QuietEventSource.opened).toHaveLength(2);
    expect(events.state().phase).toBe('connecting');
  });

  it('frames are dispatched by type and typed, and an unparsable one is dropped and counted', async () => {
    const { http, events, boot } = configure();
    const received = listen(events);
    events.subscribe();
    await booted(http, boot);
    last().open();

    last().emit('task.progress', PROGRESS);
    last().emit('notification', NOTE);
    last().emit('data.invalidate', EVENT, String(EVENT.seq));
    last().emit('meta.invalidate', { meta: 'm-next' });
    http.expectOne({ method: 'GET', url: '/api/v1/boot' }).flush(null, {
      status: 304,
      statusText: 'Not Modified',
    });
    expect(received).toEqual({
      task: [PROGRESS],
      notification: [NOTE],
      data: [EVENT],
      meta: [{ meta: 'm-next' }],
    });
    expect(events.dropped()).toBe(0);

    last().emit('task.progress', { ...PROGRESS, state: 'flying' });
    last().emit('task.progress', { ...PROGRESS, done: '40' });
    last().emit('notification', { ...NOTE, level: 'loud' });
    last().emit('data.invalidate', { not: 'an event' });
    last().emit('data.invalidate', { ...EVENT, seq: '8812' });
    last().emit('meta.invalidate', { meta: 7 });
    last().emit('meta.invalidate', []);
    last().emitRaw('task.progress', '{not json');
    expect(events.dropped()).toBe(8);
    expect(received.task).toHaveLength(1);
    expect(received.notification).toHaveLength(1);
    expect(received.data).toHaveLength(1);
    expect(received.meta).toHaveLength(1);

    expect(isTaskProgress({ ...PROGRESS, total: null, message: 'half way' })).toBe(true);
    expect(isTaskProgress({ id: 't', function: 'f', state: 'queued', done: 0 })).toBe(true);
    expect(isNotificationFrame({ id: 'n', key: 'k', level: 'info', createdAt: 'now' })).toBe(true);
    expect(isEvent({ ...EVENT, actor: undefined, subject: undefined, data: undefined })).toBe(true);
    expect(isMetaInvalidate({ meta: null })).toBe(true);
    expect(isMetaInvalidate({})).toBe(false);
  });

  it('the last event id is kept and a self-made reconnect passes it as after', async () => {
    const { http, events, boot } = configure();
    events.subscribe();
    await booted(http, boot);
    jest.useFakeTimers();
    last().open();
    expect(events.state()).toEqual({ phase: 'open', lastEventId: null, reconnects: 0 });

    last().emit('data.invalidate', EVENT, '8812');
    expect(events.state().lastEventId).toBe('8812');
    last().emit('task.progress', PROGRESS);
    expect(events.state().lastEventId).toBe('8812');
    last().emit('data.invalidate', { ...EVENT, seq: 8813 }, '8813');
    last().emit('data.invalidate', { not: 'an event' }, '8814');
    expect(events.state().lastEventId).toBe('8814');

    last().fail();
    expect(events.state()).toEqual({ phase: 'closed', lastEventId: '8814', reconnects: 0 });
    jest.advanceTimersByTime(MIN_BACKOFF_MS);
    expect(QuietEventSource.opened).toHaveLength(2);
    expect(last().url).toBe('/api/v1/events?after=8814');
    expect(last().init).toEqual({ withCredentials: true });
    expect(events.state()).toEqual({ phase: 'connecting', lastEventId: '8814', reconnects: 1 });

    last().open();
    last().drop();
    expect(last().readyState).toBe(CONNECTING);
    expect(events.state()).toEqual({ phase: 'connecting', lastEventId: '8814', reconnects: 2 });
    expect(QuietEventSource.opened).toHaveLength(2);
    last().open();
    expect(events.state().phase).toBe('open');

    events.close();
    expect(last().readyState).toBe(CLOSED);
    expect(events.state()).toEqual({ phase: 'idle', lastEventId: null, reconnects: 0 });
    events.subscribe();
    expect(QuietEventSource.opened).toHaveLength(3);
    expect(last().url).toBe('/api/v1/events');
  });

  it('meta.invalidate emits metaChanged, drops the descriptor cache and refetches boot once', async () => {
    const { http, events, boot } = configure();
    const client = TestBed.inject(KeelClient);
    const descriptors = TestBed.inject(KeelDescriptors);
    const versions: string[] = [];
    client.metaChanged.subscribe((version) => versions.push(version));
    events.subscribe();
    await booted(http, boot);
    last().open();

    const first = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    await first;
    await descriptors.get('dns.zone.list');
    http.expectNone('/api/v1/functions/dns.zone.list');

    last().emit('meta.invalidate', { meta: 'm-next' });
    expect(versions).toEqual(['m-next']);
    expect(client.metaVersion()).toBe('m-next');
    const refetch = http.expectOne({ method: 'GET', url: '/api/v1/boot' });
    expect(refetch.request.headers.get('If-None-Match')).toBe(`"${BOOT.meta}"`);
    const changed = { ...BOOT, meta: 'm-next', views: ['hosting.file-manager'] };
    refetch.flush(changed, { headers: { ETag: '"m-next"' } });
    await settled();
    http.expectNone('/api/v1/boot');
    expect(boot.views()).toEqual(['hosting.file-manager']);
    expect(boot.meta()).toBe('m-next');

    const again = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    await again;

    last().emit('meta.invalidate', { meta: null });
    expect(versions).toEqual(['m-next', 'm-next']);
    http
      .expectOne({ method: 'GET', url: '/api/v1/boot' })
      .flush(null, { status: 304, statusText: 'Not Modified' });
    await settled();
    expect(boot.views()).toEqual(['hosting.file-manager']);
    const third = descriptors.get('dns.zone.list');
    http.expectOne('/api/v1/functions/dns.zone.list').flush(LIST);
    await third;
  });

  it('the backoff doubles from one second to thirty and resets on open', async () => {
    const { http, events, boot } = configure();
    const received = listen(events);
    events.subscribe();
    await booted(http, boot);
    jest.useFakeTimers();

    let expected = MIN_BACKOFF_MS;
    for (let attempt = 0; attempt < 7; attempt += 1) {
      const before = QuietEventSource.opened.length;
      last().fail();
      expect(events.state().phase).toBe('closed');
      expect(last().readyState).toBe(CLOSED);
      jest.advanceTimersByTime(expected - 1);
      expect(QuietEventSource.opened).toHaveLength(before);
      jest.advanceTimersByTime(1);
      expect(QuietEventSource.opened).toHaveLength(before + 1);
      expect(events.state().phase).toBe('connecting');
      expect(events.state().reconnects).toBe(attempt + 1);
      expected = Math.min(expected * 2, MAX_BACKOFF_MS);
    }
    expect(expected).toBe(MAX_BACKOFF_MS);
    expect(received).toEqual({ task: [], notification: [], data: [], meta: [] });

    last().open();
    expect(events.state().phase).toBe('open');
    const opened = QuietEventSource.opened.length;
    last().fail();
    jest.advanceTimersByTime(MIN_BACKOFF_MS - 1);
    expect(QuietEventSource.opened).toHaveLength(opened);
    jest.advanceTimersByTime(1);
    expect(QuietEventSource.opened).toHaveLength(opened + 1);

    events.close();
    jest.advanceTimersByTime(MAX_BACKOFF_MS);
    expect(QuietEventSource.opened).toHaveLength(opened + 1);
    expect(events.state().phase).toBe('idle');
  });
});
