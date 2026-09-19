import { DestroyRef, Injectable, computed, inject, signal } from '@angular/core';
import type { Envelope, Id, ListQuery, Page } from '@globalart/keel/meta';
import { Subscription, auditTime, filter, firstValueFrom } from 'rxjs';
import { KeelClient } from './client';
import { KeelDescriptors } from './descriptors';
import { KeelEvents } from './events';
import { mayCall } from './notifications';

export const PROBLEM_LIST = 'notify.problem.list';
export const PROBLEM_EVENT_PREFIX = 'notify.problem.';
export const PROBLEM_PAGE_SIZE = 200;
export const PROBLEM_REFRESH_MS = 250;

export type Severity = 'warning' | 'critical';

export interface Problem {
  id: Id;
  key: string;
  subject: string;
  severity: Severity;
  params: unknown;
  accountId?: Id | null;
  source: string;
  count: number;
  open: boolean;
  raisedAt: string;
  updatedAt: string;
  clearedAt?: string | null;
}

export function openProblems(pageSize = PROBLEM_PAGE_SIZE): ListQuery {
  return {
    page: 1,
    pageSize,
    sort: { key: 'raisedAt', dir: 'desc' },
    filters: [{ key: 'open', value: 'true' }],
  };
}

export function countsByKey(problems: readonly Problem[]): ReadonlyMap<string, number> {
  const counts = new Map<string, number>();
  for (const problem of problems) {
    if (problem.open) {
      counts.set(problem.key, (counts.get(problem.key) ?? 0) + 1);
    }
  }
  return counts;
}

@Injectable({ providedIn: 'root' })
export class KeelProblems {
  private readonly client = inject(KeelClient);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly events = inject(KeelEvents);
  private readonly held = signal<readonly Problem[]>([]);
  private watching: Subscription | null = null;

  readonly problems = this.held.asReadonly();
  readonly open = computed(() => countsByKey(this.held()));

  constructor() {
    inject(DestroyRef).onDestroy(() => this.stop());
  }

  available(): Promise<boolean> {
    return mayCall(this.descriptors, PROBLEM_LIST);
  }

  badge(key: string | null | undefined): number | null {
    if (key === null || key === undefined) {
      return null;
    }
    return this.open().get(key) ?? null;
  }

  async watch(): Promise<boolean> {
    if (!(await this.available())) {
      this.stop();
      return false;
    }
    this.watching ??= this.events.dataInvalidate$
      .pipe(
        filter((event) => event.type.startsWith(PROBLEM_EVENT_PREFIX)),
        auditTime(PROBLEM_REFRESH_MS),
      )
      .subscribe(() => void this.refresh());
    await this.refresh();
    return true;
  }

  async refresh(): Promise<Envelope<Page<Problem>>> {
    const envelope = await firstValueFrom(
      this.client.call<Page<Problem>>(PROBLEM_LIST, openProblems()),
    );
    if (envelope.ok) {
      this.held.set(envelope.data.data);
    }
    return envelope;
  }

  stop(): void {
    this.watching?.unsubscribe();
    this.watching = null;
    this.held.set([]);
  }
}
