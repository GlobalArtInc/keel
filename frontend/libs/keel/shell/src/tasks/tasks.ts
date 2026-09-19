import { Injectable, computed, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  KeelClient,
  KeelDescriptors,
  KeelEvents,
  TASK_CANCEL,
  TASK_GET,
  TASK_LIST,
  isActive,
  mayCall,
  recentTasks,
  type TaskDetail,
  type TaskRow,
} from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import { lookup, type Envelope, type Id, type Messages, type Page } from '@globalart/keel/meta';
import { firstValueFrom, type Observable } from 'rxjs';
import type { TaskProgress } from '@globalart/keel/meta';

export const TASKS_TITLE_KEY = 'keel.shell.tasks.title';
export const TASKS_EMPTY_KEY = 'keel.shell.tasks.empty';
export const TASKS_LOG_KEY = 'keel.shell.tasks.log';
export const TASKS_ID_KEY = 'keel.shell.tasks.id';
export const TASKS_REQUEST_KEY = 'keel.shell.tasks.request';
export const TASK_STATE_PREFIX = 'task.state';
export const TASK_COLUMN_PREFIX = 'task.column';
export const TITLE_SUFFIX = '.title';

export function functionLabel(messages: Messages, func: string): string {
  return lookup(messages, `${func}${TITLE_SUFFIX}`) ?? func;
}

export function activeIds(rows: readonly Pick<TaskRow, 'id' | 'state'>[]): Set<Id> {
  return new Set(rows.filter((row) => isActive(row.state)).map((row) => row.id));
}

function failed<T>(error: unknown): Envelope<T> {
  return { ok: false, error: unreachable(error) };
}

@Injectable({ providedIn: 'root' })
export class KeelTaskWatch {
  private readonly client = inject(KeelClient);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly events = inject(KeelEvents);
  private readonly active = signal<ReadonlySet<Id>>(new Set());

  readonly count = computed(() => this.active().size);
  readonly frames$: Observable<TaskProgress> = this.events.taskProgress$;

  constructor() {
    this.frames$.pipe(takeUntilDestroyed()).subscribe((frame) => this.note(frame));
  }

  available(): Promise<boolean> {
    return mayCall(this.descriptors, TASK_LIST);
  }

  async list(): Promise<Envelope<Page<TaskRow>>> {
    const envelope = await firstValueFrom(
      this.client.call<Page<TaskRow>>(TASK_LIST, recentTasks()),
    ).catch(failed<Page<TaskRow>>);
    if (envelope.ok) {
      this.active.set(activeIds(envelope.data.data));
    }
    return envelope;
  }

  detail(id: Id): Promise<Envelope<TaskDetail>> {
    return firstValueFrom(this.client.call<TaskDetail>(TASK_GET, { id })).catch(failed<TaskDetail>);
  }

  cancel(id: Id): Promise<Envelope> {
    return firstValueFrom(this.client.call(TASK_CANCEL, { id })).catch(failed);
  }

  drop(): void {
    this.active.set(new Set());
  }

  private note(frame: TaskProgress): void {
    const held = this.active();
    if (isActive(frame.state) === held.has(frame.id)) {
      return;
    }
    const next = new Set(held);
    if (isActive(frame.state)) {
      next.add(frame.id);
    } else {
      next.delete(frame.id);
    }
    this.active.set(next);
  }
}
