import { Injectable, computed, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import type {
  Envelope,
  Id,
  ListQuery,
  NotificationFrame,
  NotificationLevel,
  Page,
} from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { KeelClient } from './client';
import { KeelDescriptors } from './descriptors';
import { KeelEvents } from './events';

export const MESSAGE_LIST = 'notify.message.list';
export const MESSAGE_MARK = 'notify.message.mark';
export const MESSAGE_CLEAR = 'notify.message.clear';
export const MESSAGE_PAGE_SIZE = 50;
export const FRAME_SOURCE = 'live';

export interface Note {
  id: Id;
  key: string;
  params: unknown;
  level: NotificationLevel;
  source: string;
  problem?: Id | null;
  unread: boolean;
  readAt?: string | null;
  createdAt: string;
}

export function recentMessages(pageSize = MESSAGE_PAGE_SIZE): ListQuery {
  return { page: 1, pageSize, sort: { key: 'createdAt', dir: 'desc' } };
}

export function noteOf(frame: NotificationFrame): Note {
  return {
    id: frame.id,
    key: frame.key,
    params: frame.params ?? null,
    level: frame.level,
    source: FRAME_SOURCE,
    problem: null,
    unread: true,
    createdAt: frame.createdAt,
  };
}

export function problemKeyOf(note: Pick<Note, 'params'>): string | null {
  const params = note.params;
  if (typeof params !== 'object' || params === null || Array.isArray(params)) {
    return null;
  }
  const key = (params as Record<string, unknown>)['key'];
  return typeof key === 'string' && key !== '' ? key : null;
}

export async function mayCall(descriptors: KeelDescriptors, func: string): Promise<boolean> {
  try {
    return (await descriptors.list()).some((descriptor) => descriptor.name === func);
  } catch {
    return false;
  }
}

@Injectable({ providedIn: 'root' })
export class KeelNotifications {
  private readonly client = inject(KeelClient);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly held = signal<readonly Note[]>([]);

  readonly messages = this.held.asReadonly();
  readonly unread = computed(() => this.held().filter((note) => note.unread).length);

  constructor() {
    inject(KeelEvents)
      .notification$.pipe(takeUntilDestroyed())
      .subscribe((frame) => this.prepend(noteOf(frame)));
  }

  available(): Promise<boolean> {
    return mayCall(this.descriptors, MESSAGE_LIST);
  }

  async refresh(): Promise<Envelope<Page<Note>>> {
    const envelope = await firstValueFrom(
      this.client.call<Page<Note>>(MESSAGE_LIST, recentMessages()),
    );
    if (envelope.ok) {
      this.held.set(envelope.data.data);
    }
    return envelope;
  }

  async mark(ids: readonly Id[]): Promise<Envelope> {
    if (ids.length === 0) {
      return { ok: true, data: { marked: 0 }, meta: { requestId: '' } };
    }
    const envelope = await firstValueFrom(this.client.call(MESSAGE_MARK, { ids: [...ids] }));
    if (envelope.ok) {
      this.held.update((notes) =>
        notes.map((note) => (ids.includes(note.id) ? { ...note, unread: false } : note)),
      );
    }
    return envelope;
  }

  async clear(): Promise<Envelope> {
    const envelope = await firstValueFrom(this.client.call(MESSAGE_CLEAR, {}));
    if (envelope.ok) {
      this.held.update((notes) => notes.filter((note) => note.unread));
    }
    return envelope;
  }

  dismiss(id: Id): void {
    this.held.update((notes) => notes.filter((note) => note.id !== id));
  }

  drop(): void {
    this.held.set([]);
  }

  private prepend(note: Note): void {
    this.held.update((notes) => [note, ...notes.filter((held) => held.id !== note.id)]);
  }
}
