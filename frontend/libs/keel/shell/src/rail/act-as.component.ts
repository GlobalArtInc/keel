import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Envelope, Id, Level, ListQuery, Page } from '@globalart/keel/meta';
import { describeError } from '@globalart/keel/renderer';
import {
  PhxBannerComponent,
  PhxEmptyComponent,
  PhxLoaderComponent,
  PhxModalRef,
  PhxSearchbarComponent,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { levelLabel } from '../layout/sidebar';
import { KeelSu } from './su';

export const PRINCIPAL_LIST = 'keel.principal.list';
export const ACT_AS_TITLE_KEY = 'keel.shell.act_as.title';
export const ACT_AS_SEARCH_KEY = 'keel.shell.act_as.search';
export const ACT_AS_ENTER_KEY = 'keel.shell.act_as.enter';
export const ACT_AS_EMPTY_KEY = 'keel.shell.act_as.empty';
export const ACT_AS_PAGE_SIZE = 20;

export interface PrincipalRow {
  id: Id;
  login: string;
  level: Level;
}

export type ActAsState =
  | { kind: 'idle' }
  | { kind: 'searching' }
  | { kind: 'found'; rows: PrincipalRow[] }
  | { kind: 'failed'; message: string };

export function principalQuery(search: string): ListQuery {
  const query: ListQuery = { page: 1, pageSize: ACT_AS_PAGE_SIZE };
  if (search !== '') {
    query.search = search;
  }
  return query;
}

@Component({
  selector: 'keel-act-as',
  imports: [PhxSearchbarComponent, PhxBannerComponent, PhxEmptyComponent, PhxLoaderComponent],
  template: `
    <phx-searchbar
      class="search"
      block
      [collapsible]="false"
      [placeholder]="searchLabel()"
      [label]="searchLabel()"
      [disabled]="entering()"
      (search)="find($event)"
    />
    @switch (state().kind) {
      @case ('searching') {
        <div class="loading" role="status">
          <phx-loader [size]="20" />
        </div>
      }
      @case ('failed') {
        <phx-banner status="danger" role="alert">{{ failure() }}</phx-banner>
      }
      @case ('found') {
        @if (rows().length === 0) {
          <phx-empty compact icon="user" [title]="emptyText()" />
        } @else {
          <ul class="rows" [attr.aria-label]="title()">
            @for (row of rows(); track row.id) {
              <li>
                <button
                  type="button"
                  class="row"
                  [attr.data-principal]="row.id"
                  [disabled]="entering()"
                  (click)="enter(row)"
                >
                  <span class="login">{{ row.login }}</span>
                  <span class="level">{{ levelOf(row.level) }}</span>
                  <span class="enter">{{ enterLabel() }}</span>
                </button>
              </li>
            }
          </ul>
        }
      }
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 16px 0;
    }
    .rows {
      display: flex;
      flex-direction: column;
      gap: 2px;
      padding: 0;
      margin: 0;
      list-style: none;
    }
    .row {
      display: flex;
      gap: 12px;
      align-items: center;
      width: 100%;
      padding: 8px 12px;
      font: var(--phx-text-14);
      color: var(--phx-ink);
      text-align: left;
      cursor: pointer;
      background-color: transparent;
      border: 0;
      border-radius: var(--phx-radius-medium);
      outline: none;
      transition: background-color var(--phx-transition-fast);
    }
    .row:hover,
    .row:focus-visible {
      background-color: var(--phx-ground);
    }
    .row:focus-visible {
      outline: var(--phx-focus-ring);
      outline-offset: -2px;
    }
    .row:disabled {
      color: var(--phx-ink-disabled);
      cursor: default;
    }
    .login {
      flex: 1;
      min-width: 0;
      overflow: hidden;
      font: var(--phx-heading-14);
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .level {
      color: var(--phx-ink-muted);
    }
    .enter {
      color: var(--phx-accent-text);
    }
  `,
  host: {
    '[attr.data-state]': 'state().kind',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelActAsComponent {
  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly su = inject(KeelSu);
  private readonly ref = inject(PhxModalRef);
  private readonly held = signal<ActAsState>({ kind: 'idle' });
  private readonly busy = signal(false);
  private searches = 0;

  readonly state = this.held.asReadonly();
  readonly entering = this.busy.asReadonly();
  readonly rows = computed(() => {
    const state = this.held();
    return state.kind === 'found' ? state.rows : [];
  });

  protected readonly title = computed(() => this.i18n.t(ACT_AS_TITLE_KEY));
  protected readonly searchLabel = computed(() => this.i18n.t(ACT_AS_SEARCH_KEY));
  protected readonly enterLabel = computed(() => this.i18n.t(ACT_AS_ENTER_KEY));
  protected readonly emptyText = computed(() => this.i18n.t(ACT_AS_EMPTY_KEY));
  protected readonly failure = computed(() => {
    const state = this.held();
    return state.kind === 'failed' ? state.message : '';
  });

  levelOf(level: Level): string {
    return levelLabel(this.boot.products(), level, this.i18n.messages(), (key) => this.i18n.t(key));
  }

  async find(search: string | Event): Promise<void> {
    if (search instanceof Event) {
      return;
    }
    const attempt = ++this.searches;
    this.held.set({ kind: 'searching' });
    const envelope = await this.call<Page<PrincipalRow>>(PRINCIPAL_LIST, principalQuery(search));
    if (attempt !== this.searches) {
      return;
    }
    this.held.set(
      envelope.ok
        ? { kind: 'found', rows: envelope.data.data }
        : { kind: 'failed', message: describeError(this.i18n, envelope.error) },
    );
  }

  async enter(row: PrincipalRow): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.busy.set(true);
    try {
      const envelope = await this.su.enter(row.id);
      if (envelope.ok) {
        this.ref.close(true);
      } else {
        this.held.set({ kind: 'failed', message: describeError(this.i18n, envelope.error) });
      }
    } catch (error: unknown) {
      this.held.set({ kind: 'failed', message: describeError(this.i18n, unreachable(error)) });
    } finally {
      this.busy.set(false);
    }
  }

  private call<T>(func: string, params: unknown): Promise<Envelope<T>> {
    return firstValueFrom(this.client.call<T>(func, params)).catch(
      (error: unknown): Envelope<T> => ({ ok: false, error: unreachable(error) }),
    );
  }
}
