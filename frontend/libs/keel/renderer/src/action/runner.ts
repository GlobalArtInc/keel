import { Injectable, inject } from '@angular/core';
import { KeelClient, KeelI18n, isTaskRef } from '@globalart/keel/client';
import { newIdempotencyKey, unreachable } from '@globalart/keel/forms';
import { entityPrefix, type Action, type Descriptor, type Envelope } from '@globalart/keel/meta';
import { PhxConfirmService, PhxToastService, type PhxConfirmOptions } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { CONFIRM_COUNT_KEY, CONFIRM_MANY_KEY, CONFIRM_ONE_KEY, describeError } from '../messages';
import { KeelTasks } from '../tasks/follow';

export const ROW_PLACEHOLDER = '$id';
export const DEFAULT_KEY = 'id';

export type Row = Record<string, unknown>;

export interface ActionRequest {
  descriptor: Descriptor;
  action: Action;
  rows?: readonly Row[];
}

export type ActionOutcome<T = unknown> = Envelope<T> | null;

export function keyOf(descriptor: Descriptor): string {
  return descriptor.key ?? DEFAULT_KEY;
}

export function labelKeyOf(descriptor: Descriptor, action: Action): string {
  return `${entityPrefix(descriptor.name)}.action.${action.id}`;
}

function confirmKeyOf(descriptor: Descriptor, action: Action): string {
  return `${entityPrefix(descriptor.name)}.confirm.${action.id}`;
}

export function idsOf(descriptor: Descriptor, rows: readonly Row[]): string[] {
  const key = keyOf(descriptor);
  return rows.map((row) => String(row[key]));
}

export function substitute(
  params: Record<string, unknown>,
  id: string | undefined,
): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(params)) {
    body[name] = value === ROW_PLACEHOLDER && id !== undefined ? id : value;
  }
  return body;
}

export function bodyOf(request: ActionRequest): Record<string, unknown> {
  const rows = request.rows ?? [];
  const role = request.action.role;
  if (role === 'delete' || role === 'group') {
    return { ids: idsOf(request.descriptor, rows) };
  }
  const [first] = idsOf(request.descriptor, rows);
  return substitute(request.action.params ?? {}, first);
}

@Injectable({ providedIn: 'root' })
export class KeelActionRunner {
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly confirms = inject(PhxConfirmService);
  private readonly toasts = inject(PhxToastService);
  private readonly tasks = inject(KeelTasks);

  label(request: ActionRequest): string {
    return this.i18n.t(labelKeyOf(request.descriptor, request.action));
  }

  async run<T = unknown>(request: ActionRequest): Promise<ActionOutcome<T>> {
    const func = request.action.function;
    if (func === null || func === undefined) {
      throw new TypeError(`the action ${request.action.id} names no function`);
    }
    const label = this.label(request);
    if (!(await this.confirmed(request, label))) {
      return null;
    }
    const envelope = await firstValueFrom(
      this.client.call<T>(func, bodyOf(request), { idempotencyKey: newIdempotencyKey() }),
    ).catch((error: unknown): Envelope<T> => ({ ok: false, error: unreachable(error) }));
    if (!envelope.ok) {
      this.toasts.danger(describeError(this.i18n, envelope.error), { title: label });
      return envelope;
    }
    if (isTaskRef(envelope.data)) {
      return this.tasks.follow<T>(envelope.data, { title: label });
    }
    return envelope;
  }

  private async confirmed(request: ActionRequest, label: string): Promise<boolean> {
    const confirm = request.action.confirm ?? false;
    if (confirm === false) {
      return true;
    }
    const rows = request.rows ?? [];
    const count = rows.length;
    const options: PhxConfirmOptions = {
      title: label,
      text: this.confirmText(request, label, count),
      confirmLabel: label,
    };
    if (confirm !== true) {
      options.destructive = confirm.destructive ?? false;
      const items = confirm.items ?? null;
      if (items !== null) {
        options.items = rows.map((row) => String(row[items] ?? ''));
      }
      if (confirm.input === 'count') {
        options.input = {
          label: this.i18n.t(CONFIRM_COUNT_KEY, { count }),
          expected: String(count),
        };
      }
    }
    return this.confirms.confirm(options);
  }

  private confirmText(request: ActionRequest, label: string, count: number): string {
    const own = confirmKeyOf(request.descriptor, request.action);
    if (this.i18n.has(own)) {
      const ids = idsOf(request.descriptor, request.rows ?? []);
      return this.i18n.t(own, { action: label, count, id: ids.join(', ') });
    }
    return count > 1
      ? this.i18n.t(CONFIRM_MANY_KEY, { action: label, count })
      : this.i18n.t(CONFIRM_ONE_KEY, { action: label });
  }
}
