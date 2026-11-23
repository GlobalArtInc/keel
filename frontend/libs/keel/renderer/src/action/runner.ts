import { DOCUMENT } from '@angular/common';
import { Injectable, InjectionToken, inject } from '@angular/core';
import { Router } from '@angular/router';
import { KeelClient, KeelI18n, isTaskRef } from '@globalart/keel/client';
import { newIdempotencyKey, unreachable } from '@globalart/keel/forms';
import { entityPrefix, type Action, type Descriptor, type Envelope } from '@globalart/keel/meta';
import { PhxConfirmService, PhxToastService, type PhxConfirmOptions } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import {
  CONFIRM_COUNT_KEY,
  CONFIRM_MANY_KEY,
  CONFIRM_ONE_KEY,
  TARGET_KEY,
  describeError,
} from '../messages';
import { KeelDownloads } from '../tasks/download';
import { KeelTasks } from '../tasks/follow';

export const ROW_PLACEHOLDER = '$id';
export const SCREEN_PARAM_PREFIX = '$screen.params.';
export const DEFAULT_KEY = 'id';

export type Row = Record<string, unknown>;

export interface ActionRequest {
  descriptor: Descriptor;
  action: Action;
  rows?: readonly Row[];
}

export type ActionOutcome<T = unknown> = Envelope<T> | null;

export type OpenExternal = (url: string) => void;

export const KEEL_OPEN_EXTERNAL = new InjectionToken<OpenExternal>('KEEL_OPEN_EXTERNAL', {
  providedIn: 'root',
  factory: () => {
    const view = inject(DOCUMENT).defaultView;
    return (url: string) => view?.location.assign(url);
  },
});

const ADDRESS = /^https?:\/\//i;

export function isLocalPath(url: string): boolean {
  return url.startsWith('/') && !url.startsWith('//') && !url.includes('\\');
}

function plain(url: string): boolean {
  return [...url].every((char) => {
    const code = char.charCodeAt(0);
    return code > 0x20 && code !== 0x7f;
  });
}

export function targetOf(data: unknown): string | null {
  if (typeof data !== 'object' || data === null) {
    return null;
  }
  const url = (data as { url?: unknown }).url;
  if (typeof url !== 'string' || !plain(url)) {
    return null;
  }
  return isLocalPath(url) || ADDRESS.test(url) ? url : null;
}

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

export interface TokenScope {
  row?: Row;
  key: string;
  screen: Record<string, unknown>;
}

export function resolveTokens(
  params: Record<string, unknown>,
  scope: TokenScope,
): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(params)) {
    const resolved = tokenValue(value, scope);
    if (resolved !== undefined) {
      body[name] = resolved;
    }
  }
  return body;
}

function tokenValue(value: unknown, scope: TokenScope): unknown {
  if (typeof value !== 'string' || !value.startsWith('$')) {
    return value;
  }
  if (value.startsWith(SCREEN_PARAM_PREFIX)) {
    const current = scope.screen[value.slice(SCREEN_PARAM_PREFIX.length)];
    return current === null ? undefined : current;
  }
  const field = value === ROW_PLACEHOLDER ? scope.key : value.slice(1);
  return scope.row?.[field];
}

export function bodyOf(request: ActionRequest): Record<string, unknown> {
  const rows = request.rows ?? [];
  const role = request.action.role;
  if (role === 'delete' || role === 'group') {
    return { ids: idsOf(request.descriptor, rows) };
  }
  return resolveTokens(request.action.params ?? {}, {
    row: rows[0],
    key: keyOf(request.descriptor),
    screen: {},
  });
}

@Injectable({ providedIn: 'root' })
export class KeelActionRunner {
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly confirms = inject(PhxConfirmService);
  private readonly toasts = inject(PhxToastService);
  private readonly tasks = inject(KeelTasks);
  private readonly downloads = inject(KeelDownloads);
  private readonly router = inject(Router);
  private readonly openExternal = inject(KEEL_OPEN_EXTERNAL);

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
    if (request.action.role === 'download') {
      await this.downloads.save({ function: func, params: bodyOf(request) }, label);
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
    if (request.action.role === 'external') {
      await this.open(envelope.data, label);
    }
    return envelope;
  }

  private async open(data: unknown, label: string): Promise<void> {
    const target = targetOf(data);
    if (target === null) {
      this.toasts.danger(this.i18n.t(TARGET_KEY), { title: label });
      return;
    }
    if (isLocalPath(target)) {
      await this.router.navigateByUrl(target);
      return;
    }
    this.openExternal(target);
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
