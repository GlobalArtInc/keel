import { computed, signal, type Signal } from '@angular/core';
import type { KeelClient } from '@globalart/keel/client';
import type { CallError, Choice, Choices, OptionsQuery } from '@globalart/keel/meta';
import { PHX_ICON_NAMES, type PhxIconName, type PhxSelectOption } from '@globalart/phoenix-ui';
import type { Subscription } from 'rxjs';
import { unreachable } from './submit';

export const OPTIONS_LIMIT = 20;

export type OptionsScope = Record<string, unknown>;

export function optionsQuery(
  search: string,
  scope: OptionsScope,
  cursor: string | null = null,
): OptionsQuery {
  const query: OptionsQuery = { limit: OPTIONS_LIMIT };
  if (search !== '') {
    query.search = search;
  }
  if (Object.keys(scope).length > 0) {
    query.scope = scope;
  }
  if (cursor !== null) {
    query.cursor = cursor;
  }
  return query;
}

export function isIconName(value: unknown): value is PhxIconName {
  return typeof value === 'string' && (PHX_ICON_NAMES as readonly string[]).includes(value);
}

export function toSelectOption(choice: Choice): PhxSelectOption<string> {
  const option: PhxSelectOption<string> = { value: choice.value, label: choice.label };
  if (choice.hint) {
    option.hint = choice.hint;
  }
  if (isIconName(choice.icon)) {
    option.icon = choice.icon;
  }
  if (choice.group) {
    option.group = choice.group;
  }
  return option;
}

export class RemoteOptions {
  private readonly held = signal<readonly Choice[]>([]);
  private readonly busy = signal(false);
  private readonly rest = signal(false);
  private readonly failed = signal<CallError | null>(null);
  private query = '';
  private pending: Subscription | null = null;

  readonly choices = this.held.asReadonly();
  readonly loading = this.busy.asReadonly();
  readonly more = this.rest.asReadonly();
  readonly failure = this.failed.asReadonly();
  readonly options: Signal<PhxSelectOption<string>[]> = computed(() =>
    this.held().map(toSelectOption),
  );

  constructor(
    private readonly client: KeelClient,
    readonly func: string,
    private readonly scope: OptionsScope,
  ) {}

  search(query: string): void {
    this.query = query;
    this.fetch(null);
  }

  loadMore(): void {
    if (!this.rest() || this.busy()) {
      return;
    }
    const last = this.held().at(-1);
    this.fetch(last === undefined ? null : last.value);
  }

  replace(choices: readonly Choice[]): void {
    this.cancel();
    this.held.set([...choices]);
    this.rest.set(false);
  }

  destroy(): void {
    this.cancel();
  }

  private fetch(cursor: string | null): void {
    this.cancel();
    this.busy.set(true);
    this.failed.set(null);
    this.pending = this.client
      .call<Choices>(this.func, optionsQuery(this.query, this.scope, cursor))
      .subscribe({
        next: (envelope) => {
          if (!envelope.ok) {
            this.failed.set(envelope.error);
            return;
          }
          this.held.set(
            cursor === null ? envelope.data.data : [...this.held(), ...envelope.data.data],
          );
          this.rest.set(envelope.data.more);
        },
        error: (error: unknown) => {
          this.failed.set(unreachable(error));
          this.busy.set(false);
          this.pending = null;
        },
        complete: () => {
          this.busy.set(false);
          this.pending = null;
        },
      });
  }

  private cancel(): void {
    if (this.pending !== null) {
      this.pending.unsubscribe();
      this.pending = null;
    }
    this.busy.set(false);
  }
}
