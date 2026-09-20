import { DOCUMENT } from '@angular/common';
import { Injectable, inject } from '@angular/core';
import { KeelClient, KeelI18n, type DownloadResult, type Scope } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Action, ExportFormat, ListQuery } from '@globalart/keel/meta';
import { PhxToastService, type PhxColumnFilter, type PhxSort } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { describeError } from '../messages';
import { handToBrowser } from '../tasks/download';
import { KeelTasks } from '../tasks/follow';
import { EXPORT_FUNCTION } from './actions';

export const DEFAULT_FORMATS: readonly ExportFormat[] = ['csv'];
export const FORMAT_KEY_PREFIX = 'keel.export.format.';

export interface TableState {
  search: string;
  sort: PhxSort | null;
  filters: readonly PhxColumnFilter[];
}

export interface ExportRequest {
  func: string;
  query: ListQuery;
  columns: string[];
  format: ExportFormat;
}

export type ExportOutcome =
  { kind: 'file' } | { kind: 'task'; done: boolean } | { kind: 'error' } | { kind: 'cancelled' };

export function formatsOf(action: Action): readonly ExportFormat[] {
  const formats = action.formats ?? [];
  return formats.length > 0 ? formats : DEFAULT_FORMATS;
}

export function formatKeyOf(format: ExportFormat): string {
  return `${FORMAT_KEY_PREFIX}${format}`;
}

export function exportQueryOf(table: TableState, scope: Scope): ListQuery {
  const query: ListQuery = {};
  if (table.search !== '') {
    query.search = table.search;
  }
  if (table.sort !== null) {
    query.sort = { key: table.sort.key, dir: table.sort.dir };
  }
  if (table.filters.length > 0) {
    query.filters = table.filters.map(({ key, value }) => ({ key, value }));
  }
  if (Object.keys(scope).length > 0) {
    query.scope = scope;
  }
  return query;
}

@Injectable({ providedIn: 'root' })
export class KeelExports {
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly toasts = inject(PhxToastService);
  private readonly tasks = inject(KeelTasks);
  private readonly document = inject(DOCUMENT);

  async run(request: ExportRequest, title: string): Promise<ExportOutcome> {
    const result = await firstValueFrom(this.client.download(EXPORT_FUNCTION, request)).catch(
      (error: unknown): DownloadResult => ({ ok: false, error: unreachable(error) }),
    );
    if (!result.ok) {
      this.toasts.danger(describeError(this.i18n, result.error), { title });
      return { kind: 'error' };
    }
    if ('task' in result) {
      const outcome = await this.tasks.follow(result.task, { title });
      return outcome === null ? { kind: 'cancelled' } : { kind: 'task', done: outcome.ok };
    }
    handToBrowser(result.file, this.document);
    return { kind: 'file' };
  }
}
