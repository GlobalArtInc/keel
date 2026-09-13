import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import {
  KEEL_EVENTS_PATH,
  KEEL_EVENT_SOURCE,
  KeelEvents,
  MAX_BACKOFF_MS,
  MIN_BACKOFF_MS,
  type EventSourceLike,
} from '@globalart/keel/client';
import type { Event as KeelEvent } from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const EVENT = JSON.parse(readFileSync(join(FIXTURES, 'event.json'), 'utf8')) as KeelEvent;

const CONNECTING = 0;
const OPEN = 1;
const CLOSED = 2;

class FakeEventSource extends EventTarget implements EventSourceLike {
  static opened: FakeEventSource[] = [];
  readyState = CONNECTING;

  constructor(
    readonly url: string,
    readonly init: EventSourceInit,
  ) {
    super();
    FakeEventSource.opened.push(this);
  }

  static last(): FakeEventSource {
    const last = FakeEventSource.opened.at(-1);
    if (last === undefined) {
      throw new Error('no event source was opened');
    }
    return last;
  }

  open(): void {
    this.readyState = OPEN;
    this.dispatchEvent(new Event('open'));
  }

  refuse(): void {
    this.readyState = CLOSED;
    this.dispatchEvent(new Event('error'));
  }

  drop(): void {
    this.readyState = CONNECTING;
    this.dispatchEvent(new Event('error'));
  }

  frame(type: string, data: unknown, raw = false): void {
    this.dispatchEvent(
      new MessageEvent(type, {
        data: raw ? String(data) : JSON.stringify(data),
        lastEventId: '8812',
      }),
    );
  }

  close(): void {
    this.readyState = CLOSED;
  }
}

describe('KeelEvents', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    FakeEventSource.opened = [];
    TestBed.configureTestingModule({
      providers: [
        {
          provide: KEEL_EVENT_SOURCE,
          useValue: (url: string, init: EventSourceInit) => new FakeEventSource(url, init),
        },
      ],
    });
  });

  afterEach(() => {
    TestBed.inject(KeelEvents).close();
    jest.useRealTimers();
  });

  it('the stub reconnects with a doubling backoff and dispatches frames by type', () => {
    const events = TestBed.inject(KeelEvents);
    const received: Record<string, unknown[]> = { task: [], notification: [], data: [], meta: [] };
    events.taskProgress$.subscribe((frame) => received['task'].push(frame));
    events.notification$.subscribe((frame) => received['notification'].push(frame));
    events.dataInvalidate$.subscribe((frame) => received['data'].push(frame));
    events.metaInvalidate$.subscribe((frame) => received['meta'].push(frame));

    expect(events.state()).toBe('idle');
    events.subscribe();
    events.subscribe();
    expect(FakeEventSource.opened).toHaveLength(1);
    expect(FakeEventSource.last().url).toBe(TestBed.inject(KEEL_EVENTS_PATH));
    expect(FakeEventSource.last().url).toBe('/api/v1/events');
    expect(FakeEventSource.last().init).toEqual({ withCredentials: true });
    expect(events.state()).toBe('connecting');

    FakeEventSource.last().open();
    expect(events.state()).toBe('open');
    FakeEventSource.last().frame('task.progress', { task: 't-1', percent: 40 });
    FakeEventSource.last().frame('notification', { id: 'n-1' });
    FakeEventSource.last().frame('data.invalidate', EVENT);
    FakeEventSource.last().frame('data.invalidate', { not: 'an event' });
    FakeEventSource.last().frame('meta.invalidate', { meta: 'm-next' });
    FakeEventSource.last().frame('task.progress', '{not json', true);
    expect(received).toEqual({
      task: [{ task: 't-1', percent: 40 }],
      notification: [{ id: 'n-1' }],
      data: [EVENT],
      meta: [{ meta: 'm-next' }],
    });

    let expected = MIN_BACKOFF_MS;
    for (let attempt = 0; attempt < 7; attempt += 1) {
      const before = FakeEventSource.opened.length;
      FakeEventSource.last().refuse();
      expect(events.state()).toBe('closed');
      jest.advanceTimersByTime(expected - 1);
      expect(FakeEventSource.opened).toHaveLength(before);
      jest.advanceTimersByTime(1);
      expect(FakeEventSource.opened).toHaveLength(before + 1);
      expect(events.state()).toBe('connecting');
      expected = Math.min(expected * 2, MAX_BACKOFF_MS);
    }
    expect(expected).toBe(MAX_BACKOFF_MS);

    FakeEventSource.last().open();
    expect(events.state()).toBe('open');
    FakeEventSource.last().drop();
    expect(events.state()).toBe('connecting');
    expect(FakeEventSource.last().readyState).toBe(CONNECTING);
    FakeEventSource.last().open();
    const opened = FakeEventSource.opened.length;
    FakeEventSource.last().refuse();
    jest.advanceTimersByTime(MIN_BACKOFF_MS);
    expect(FakeEventSource.opened).toHaveLength(opened + 1);
  });

  it('a 404 leaves the stub closed and quiet', () => {
    const events = TestBed.inject(KeelEvents);
    const frames: unknown[] = [];
    events.taskProgress$.subscribe((frame) => frames.push(frame));
    events.dataInvalidate$.subscribe((frame) => frames.push(frame));

    events.subscribe();
    FakeEventSource.last().refuse();
    expect(events.state()).toBe('closed');
    expect(FakeEventSource.last().readyState).toBe(CLOSED);
    jest.advanceTimersByTime(MIN_BACKOFF_MS);
    expect(events.state()).toBe('connecting');
    FakeEventSource.last().refuse();
    expect(events.state()).toBe('closed');
    jest.advanceTimersByTime(MIN_BACKOFF_MS);
    expect(FakeEventSource.opened).toHaveLength(2);
    jest.advanceTimersByTime(MIN_BACKOFF_MS);
    expect(FakeEventSource.opened).toHaveLength(3);
    expect(frames).toEqual([]);

    events.close();
    expect(events.state()).toBe('idle');
    jest.advanceTimersByTime(MAX_BACKOFF_MS);
    expect(FakeEventSource.opened).toHaveLength(3);
  });
});
