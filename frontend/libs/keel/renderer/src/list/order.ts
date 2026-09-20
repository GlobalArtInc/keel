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

interface Pref {
  key: string;
  value?: unknown;
}

export function columnsKeyOf(func: string): string {
  return `${COLUMNS_PREFIX}${func}`;
}

export function orderOf(value: unknown): string[] | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const order = (value as Record<string, unknown>)[ORDER_MEMBER];
  return Array.isArray(order) && order.every((key) => typeof key === 'string') ? order : null;
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

  async load(func: string): Promise<string[] | null> {
    if (!(await this.available())) {
      return null;
    }
    const envelope = await this.call<Pref>(PREF_GET, { key: columnsKeyOf(func) });
    return envelope.ok ? orderOf(envelope.data.value) : null;
  }

  async save(func: string, order: readonly string[]): Promise<void> {
    if (!(await this.available())) {
      return;
    }
    const envelope = await this.call(PREF_SET, {
      key: columnsKeyOf(func),
      value: { [ORDER_MEMBER]: [...order] },
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
