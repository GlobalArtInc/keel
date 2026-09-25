import {
  DestroyRef,
  Injectable,
  InjectionToken,
  computed,
  effect,
  inject,
  signal,
  untracked,
} from '@angular/core';
import type {
  Event as KeelEvent,
  NotificationFrame,
  NotificationLevel,
  TaskProgress,
  TaskState,
} from '@globalart/keel/meta';
import { Subject, type Observable } from 'rxjs';
import { KeelBoot } from './boot';
import { API_PREFIX, KeelClient } from './client';
import { KEEL_EVENT_SOURCE, type EventSourceLike } from './source';

export const KEEL_EVENTS_PATH = new InjectionToken<string>('KEEL_EVENTS_PATH', {
  providedIn: 'root',
  factory: () => `${API_PREFIX}/events`,
});

export type EventsPhase = 'idle' | 'connecting' | 'open' | 'closed';

export interface EventsState {
  phase: EventsPhase;
  lastEventId: string | null;
  reconnects: number;
}

export interface MetaInvalidate {
  meta: string | null;
}

export const MIN_BACKOFF_MS = 1000;
export const MAX_BACKOFF_MS = 30000;
export const AFTER_PARAM = 'after';

export const TASK_PROGRESS = 'task.progress';
export const NOTIFICATION = 'notification';
export const DATA_INVALIDATE = 'data.invalidate';
export const META_INVALIDATE = 'meta.invalidate';

const CONNECTING = 0;
const IDLE: EventsState = { phase: 'idle', lastEventId: null, reconnects: 0 };

const TASK_STATES: Readonly<Record<TaskState, true>> = {
  queued: true,
  running: true,
  done: true,
  failed: true,
  cancelled: true,
  interrupted: true,
};

const NOTIFICATION_LEVELS: Readonly<Record<NotificationLevel, true>> = {
  info: true,
  warning: true,
  error: true,
};

@Injectable({ providedIn: 'root' })
export class KeelEvents {
  private readonly path = inject(KEEL_EVENTS_PATH);
  private readonly open = inject(KEEL_EVENT_SOURCE);
  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly taskProgress = new Subject<TaskProgress>();
  private readonly notification = new Subject<NotificationFrame>();
  private readonly dataInvalidate = new Subject<KeelEvent>();
  private readonly metaInvalidate = new Subject<MetaInvalidate>();
  private readonly armed = signal(false);
  private readonly principalId = computed(() => this.boot.principal()?.id ?? null);
  private readonly current = signal<EventsState>(IDLE);
  private readonly lost = signal(0);
  private following: string | null = null;
  private source: EventSourceLike | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private backoff = MIN_BACKOFF_MS;

  readonly state = this.current.asReadonly();
  readonly dropped = this.lost.asReadonly();
  readonly taskProgress$: Observable<TaskProgress> = this.taskProgress.asObservable();
  readonly notification$: Observable<NotificationFrame> = this.notification.asObservable();
  readonly dataInvalidate$: Observable<KeelEvent> = this.dataInvalidate.asObservable();
  readonly metaInvalidate$: Observable<MetaInvalidate> = this.metaInvalidate.asObservable();

  constructor() {
    inject(DestroyRef).onDestroy(() => this.close());
    effect(() => {
      const principal = this.armed() ? this.principalId() : null;
      untracked(() => this.follow(principal));
    });
  }

  subscribe(): void {
    this.armed.set(true);
    this.follow(this.principalId());
  }

  close(): void {
    this.armed.set(false);
    this.follow(null);
  }

  private follow(principal: string | null): void {
    if (principal === this.following) {
      return;
    }
    this.following = principal;
    this.disconnect();
    if (principal !== null) {
      this.connect();
    }
  }

  private disconnect(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.source?.close();
    this.source = null;
    this.backoff = MIN_BACKOFF_MS;
    this.current.set(IDLE);
  }

  private connect(): void {
    const after = this.current().lastEventId;
    const url =
      after === null ? this.path : `${this.path}?${AFTER_PARAM}=${encodeURIComponent(after)}`;
    this.patch({ phase: 'connecting' });
    const source = this.open(url, { withCredentials: true });
    this.source = source;
    const on = (type: string, handle: (event: Event) => void): void =>
      source.addEventListener(type, (event) => {
        if (this.source === source) {
          handle(event);
        }
      });
    on('open', () => {
      this.backoff = MIN_BACKOFF_MS;
      this.patch({ phase: 'open' });
    });
    on('error', () => this.failed(source));
    on(TASK_PROGRESS, (event) => this.forward(event, isTaskProgress, this.taskProgress));
    on(NOTIFICATION, (event) => this.forward(event, isNotificationFrame, this.notification));
    on(DATA_INVALIDATE, (event) => this.forward(event, isEvent, this.dataInvalidate));
    on(META_INVALIDATE, (event) => {
      const frame = this.forward(event, isMetaInvalidate, this.metaInvalidate);
      if (frame !== undefined) {
        this.client.invalidateMeta(frame.meta);
      }
    });
  }

  private failed(source: EventSourceLike): void {
    if (source.readyState === CONNECTING) {
      this.patch({ phase: 'connecting', reconnects: this.current().reconnects + 1 });
      return;
    }
    source.close();
    this.source = null;
    this.patch({ phase: 'closed' });
    this.timer = setTimeout(() => {
      this.timer = null;
      this.patch({ reconnects: this.current().reconnects + 1 });
      this.connect();
    }, this.backoff);
    this.backoff = Math.min(this.backoff * 2, MAX_BACKOFF_MS);
  }

  private forward<T>(
    event: Event,
    accepts: (data: unknown) => data is T,
    into: Subject<T>,
  ): T | undefined {
    this.remember(event);
    const data = parse(event);
    if (!accepts(data)) {
      this.lost.update((count) => count + 1);
      return undefined;
    }
    into.next(data);
    return data;
  }

  private remember(event: Event): void {
    const id = (event as { lastEventId?: unknown }).lastEventId;
    if (typeof id === 'string' && id !== '' && id !== this.current().lastEventId) {
      this.patch({ lastEventId: id });
    }
  }

  private patch(change: Partial<EventsState>): void {
    this.current.update((state) => ({ ...state, ...change }));
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

function isRecord(data: unknown): data is Record<string, unknown> {
  return typeof data === 'object' && data !== null && !Array.isArray(data);
}

function isText(value: unknown): value is string {
  return typeof value === 'string';
}

function isOptionalText(value: unknown): value is string | null | undefined {
  return value === undefined || value === null || typeof value === 'string';
}

function isOptionalNumber(value: unknown): value is number | null | undefined {
  return value === undefined || value === null || typeof value === 'number';
}

export function isEvent(data: unknown): data is KeelEvent {
  return (
    isRecord(data) &&
    isText(data['id']) &&
    typeof data['seq'] === 'number' &&
    isText(data['type']) &&
    isText(data['at']) &&
    isText(data['instanceId']) &&
    isText(data['module'])
  );
}

export function isTaskProgress(data: unknown): data is TaskProgress {
  return (
    isRecord(data) &&
    isText(data['id']) &&
    isText(data['function']) &&
    isText(data['state']) &&
    Object.hasOwn(TASK_STATES, data['state']) &&
    typeof data['done'] === 'number' &&
    isOptionalNumber(data['total']) &&
    isOptionalText(data['message'])
  );
}

export function isNotificationFrame(data: unknown): data is NotificationFrame {
  return (
    isRecord(data) &&
    isText(data['id']) &&
    isText(data['key']) &&
    isText(data['level']) &&
    Object.hasOwn(NOTIFICATION_LEVELS, data['level']) &&
    isText(data['createdAt'])
  );
}

export function isMetaInvalidate(data: unknown): data is MetaInvalidate {
  return isRecord(data) && (data['meta'] === null || isText(data['meta']));
}
