import { Injectable, inject } from '@angular/core';
import { KeelClient, KeelDescriptors, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Envelope } from '@globalart/keel/meta';
import { PhxToastService } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { describeError } from '../messages';

export const PREF_GET = 'settings.pref.get';
export const PREF_SET = 'settings.pref.set';
export const COLUMNS_PREFIX = 'columns.';
export const ORDER_MEMBER = 'order';
export const HIDDEN_MEMBER = 'hidden';

interface Pref {
  key: string;
  value?: unknown;
}

export interface ColumnPref {
  order: string[] | null;
  hidden: string[] | null;
}

export function columnsKeyOf(func: string): string {
  return `${COLUMNS_PREFIX}${func}`;
}

function membersOf(value: unknown, member: string): string[] | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const members = (value as Record<string, unknown>)[member];
  return Array.isArray(members) && members.every((key) => typeof key === 'string') ? members : null;
}

export function orderOf(value: unknown): string[] | null {
  return membersOf(value, ORDER_MEMBER);
}

export function hiddenOf(value: unknown): string[] | null {
  return membersOf(value, HIDDEN_MEMBER);
}

export function prefOf(value: unknown): ColumnPref {
  return { order: orderOf(value), hidden: hiddenOf(value) };
}

export function valueOf(pref: ColumnPref): Record<string, string[]> {
  const value: Record<string, string[]> = {};
  if (pref.order !== null) {
    value[ORDER_MEMBER] = [...pref.order];
  }
  if (pref.hidden !== null) {
    value[HIDDEN_MEMBER] = [...pref.hidden];
  }
  return value;
}

export function reordered(declared: readonly string[], order: readonly string[]): string[] {
  const known = new Set(declared);
  return order.filter((key) => known.has(key));
}

@Injectable({ providedIn: 'root' })
export class KeelColumnOrders {
  private readonly client = inject(KeelClient);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly i18n = inject(KeelI18n);
  private readonly toasts = inject(PhxToastService);

  async available(): Promise<boolean> {
    const index = await this.descriptors.list().catch((): [] => []);
    const names = new Set(index.map((descriptor) => descriptor.name));
    return names.has(PREF_GET) && names.has(PREF_SET);
  }

  async load(func: string): Promise<ColumnPref> {
    if (!(await this.available())) {
      return prefOf(null);
    }
    const envelope = await this.call<Pref>(PREF_GET, { key: columnsKeyOf(func) });
    return prefOf(envelope.ok ? envelope.data.value : null);
  }

  async save(func: string, pref: ColumnPref): Promise<void> {
    if (!(await this.available())) {
      return;
    }
    const envelope = await this.call(PREF_SET, {
      key: columnsKeyOf(func),
      value: valueOf(pref),
    });
    if (!envelope.ok) {
      this.toasts.danger(describeError(this.i18n, envelope.error));
    }
  }

  private call<T>(func: string, params: unknown): Promise<Envelope<T>> {
    return firstValueFrom(this.client.call<T>(func, params)).catch(
      (error: unknown): Envelope<T> => ({ ok: false, error: unreachable(error) }),
    );
  }
}
