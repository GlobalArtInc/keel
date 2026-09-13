import { DestroyRef, Injectable, InjectionToken, inject, signal } from '@angular/core';
import type { Event as KeelEvent } from '@globalart/keel/meta';
import { Subject, type Observable } from 'rxjs';
import { API_PREFIX } from './client';

export const KEEL_EVENTS_PATH = new InjectionToken<string>('KEEL_EVENTS_PATH', {
  providedIn: 'root',
  factory: () => `${API_PREFIX}/events`,
});

export interface EventSourceLike {
  readonly readyState: number;
  addEventListener(type: string, listener: (event: Event) => void): void;
  close(): void;
}

export type EventSourceFactory = (url: string, init: EventSourceInit) => EventSourceLike;

export const KEEL_EVENT_SOURCE = new InjectionToken<EventSourceFactory>('KEEL_EVENT_SOURCE', {
  providedIn: 'root',
  factory: () => (url, init) => new EventSource(url, init),
});

export type EventsState = 'idle' | 'connecting' | 'open' | 'closed';

export const MIN_BACKOFF_MS = 1000;
export const MAX_BACKOFF_MS = 30000;

export const TASK_PROGRESS = 'task.progress';
export const NOTIFICATION = 'notification';
export const DATA_INVALIDATE = 'data.invalidate';
export const META_INVALIDATE = 'meta.invalidate';

const CONNECTING = 0;

@Injectable({ providedIn: 'root' })
export class KeelEvents {
  private readonly path = inject(KEEL_EVENTS_PATH);
  private readonly open = inject(KEEL_EVENT_SOURCE);
  private readonly taskProgress = new Subject<unknown>();
  private readonly notification = new Subject<unknown>();
  private readonly dataInvalidate = new Subject<KeelEvent>();
  private readonly metaInvalidate = new Subject<unknown>();
  private source: EventSourceLike | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private backoff = MIN_BACKOFF_MS;

  readonly state = signal<EventsState>('idle');
  readonly taskProgress$: Observable<unknown> = this.taskProgress.asObservable();
  readonly notification$: Observable<unknown> = this.notification.asObservable();
  readonly dataInvalidate$: Observable<KeelEvent> = this.dataInvalidate.asObservable();
  readonly metaInvalidate$: Observable<unknown> = this.metaInvalidate.asObservable();

  constructor() {
    inject(DestroyRef).onDestroy(() => this.close());
  }

  subscribe(): void {
    if (this.source !== null || this.timer !== null) {
      return;
    }
    this.connect();
  }

  close(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.source?.close();
    this.source = null;
    this.backoff = MIN_BACKOFF_MS;
    this.state.set('idle');
  }

  private connect(): void {
    this.state.set('connecting');
    const source = this.open(this.path, { withCredentials: true });
    this.source = source;
    source.addEventListener('open', () => {
      if (this.source === source) {
        this.backoff = MIN_BACKOFF_MS;
        this.state.set('open');
      }
    });
    source.addEventListener('error', () => this.failed(source));
    source.addEventListener(TASK_PROGRESS, (event) => this.forward(this.taskProgress, event));
    source.addEventListener(NOTIFICATION, (event) => this.forward(this.notification, event));
    source.addEventListener(DATA_INVALIDATE, (event) => {
      const data = parse(event);
      if (isEvent(data)) {
        this.dataInvalidate.next(data);
      }
    });
    source.addEventListener(META_INVALIDATE, (event) => this.forward(this.metaInvalidate, event));
  }

  private failed(source: EventSourceLike): void {
    if (this.source !== source) {
      return;
    }
    if (source.readyState === CONNECTING) {
      this.state.set('connecting');
      return;
    }
    source.close();
    this.source = null;
    this.state.set('closed');
    this.timer = setTimeout(() => {
      this.timer = null;
      this.connect();
    }, this.backoff);
    this.backoff = Math.min(this.backoff * 2, MAX_BACKOFF_MS);
  }

  private forward(subject: Subject<unknown>, event: Event): void {
    const data = parse(event);
    if (data !== undefined) {
      subject.next(data);
    }
  }
}

function parse(event: Event): unknown {
  const raw = (event as { data?: unknown }).data;
  if (typeof raw !== 'string') {
    return undefined;
  }
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

function isEvent(data: unknown): data is KeelEvent {
  return (
    typeof data === 'object' &&
    data !== null &&
    typeof (data as { type?: unknown }).type === 'string' &&
    typeof (data as { seq?: unknown }).seq === 'number'
  );
}
