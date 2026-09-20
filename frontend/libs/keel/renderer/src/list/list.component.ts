import {
  ChangeDetectionStrategy,
  Component,
  Injector,
  computed,
  effect,
  inject,
  input,
  linkedSignal,
  signal,
  untracked,
} from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import {
  KeelBoot,
  KeelClient,
  KeelDates,
  KeelDescriptors,
  KeelEvents,
  KeelI18n,
  listFetch,
  type KeelListFetch,
} from '@globalart/keel/client';
import { RemoteOptions, newIdempotencyKey, unreachable } from '@globalart/keel/forms';
import type {
  Action,
  Column,
  Descriptor,
  Envelope,
  ExportFormat,
  InlineEdit,
} from '@globalart/keel/meta';
import {
  PhxButtonComponent,
  PhxDataTableComponent,
  PhxDropdownComponent,
  PhxDropdownItemComponent,
  PhxDropdownTriggerDirective,
  PhxListTableComponent,
  PhxModalService,
  PhxPageHeaderComponent,
  PhxSelectionBarComponent,
  PhxTableCellDirective,
  PhxToastService,
  PhxToggleComponent,
  type PhxButtonAppearance,
  type PhxCellChange,
  type PhxColumn,
  type PhxColumnFilter,
  type PhxDataTablePage,
  type PhxIconName,
  type PhxRowAction,
  type PhxRowActionEvent,
  type PhxRowEvent,
  type PhxSelectionAction,
  type PhxSort,
} from '@globalart/phoenix-ui';
import {
  Subject,
  catchError,
  combineLatest,
  firstValueFrom,
  from,
  map,
  merge,
  of,
  startWith,
  switchMap,
  type Observable,
} from 'rxjs';
import { KeelActionRunner, idsOf, keyOf, labelKeyOf, substitute, type Row } from '../action/runner';
import { columnsOf, totalsOf, type ColumnTotal } from '../columns';
import { describeError } from '../messages';
import { MULTI_TITLE_KEY, isMultiForm, openMultiForm } from '../multi/multi-form.component';
import { KeelNavigation } from '../navigation';
import {
  EXPORT_FUNCTION,
  actionOf,
  appearanceOf,
  bulkActionsOf,
  defaultActionOf,
  emptyActionOf,
  iconOf,
  isHidden,
  rowActionsOf,
  screenParamsOf,
  toolbarActionsOf,
  type RowScope,
} from './actions';
import { KeelExports, exportQueryOf, formatKeyOf, formatsOf } from './export';
import { invalidations } from './invalidate';
import { KeelColumnOrders, reordered } from './order';
import { scopeOf, searchOf } from './scope';

export interface ReadonlyToggle {
  key: string;
  label: string;
}

type RowKey = string | number;

export type CompactRows = { phase: 'loading' } | { phase: 'loaded'; rows: Row[]; total: number };

const DEFAULT_EMPTY_ICON: PhxIconName = 'plus';
const COMPACT_LOADING: CompactRows = { phase: 'loading' };

@Component({
  selector: 'keel-list',
  imports: [
    PhxPageHeaderComponent,
    PhxButtonComponent,
    PhxDropdownComponent,
    PhxDropdownItemComponent,
    PhxDropdownTriggerDirective,
    PhxSelectionBarComponent,
    PhxDataTableComponent,
    PhxListTableComponent,
    PhxTableCellDirective,
    PhxToggleComponent,
  ],
  template: `
    @if (compact() !== null) {
      <phx-list-table
        class="compact"
        [columns]="columns()"
        [rows]="firstRowsOf(compactRows())"
        [rowKey]="key()"
        [loading]="compactRows().phase === 'loading'"
        [skeletonRows]="compact() ?? 1"
        [footer]="false"
        [label]="title()"
        [emptyTitle]="emptyTitle()"
        [clickable]="clickable()"
        (rowClick)="onRowClick($event)"
      />
    } @else {
      <phx-page-header [title]="title()">
        @for (action of toolbar(); track action.id) {
          @if (action.role === 'export') {
            <button
              phxActions
              [phxButton]="appearanceOf(action)"
              type="button"
              [icon]="iconOf(action)"
              [attr.data-action]="action.id"
              [phxDropdownTrigger]="formats"
              [disabled]="exporting()"
            >
              {{ labelOf(action) }}
            </button>
            <phx-dropdown #formats menu align="end" [label]="labelOf(action)">
              @for (format of formatsOf(action); track format) {
                <phx-dropdown-item
                  [attr.data-format]="format"
                  (select)="formats.select(); export(action, format)"
                >
                  {{ formatLabel(format) }}
                </phx-dropdown-item>
              }
            </phx-dropdown>
          } @else {
            <button
              phxActions
              [phxButton]="appearanceOf(action)"
              type="button"
              [icon]="iconOf(action)"
              [attr.data-action]="action.id"
              (click)="run(action)"
            >
              {{ labelOf(action) }}
            </button>
          }
        }
      </phx-page-header>
      <phx-selection-bar
        [count]="selection().size"
        [actions]="bulk()"
        (action)="runBulk($event)"
        (clear)="clearSelection()"
      />
      @for (fetch of [fetch()]; track fetch) {
        <phx-data-table
          [columns]="columns()"
          [fetch]="fetch"
          [rowKey]="key()"
          [label]="title()"
          [searchable]="searchable()"
          [(search)]="tableSearch"
          [(sort)]="tableSort"
          [(filters)]="tableFilters"
          [columnOrder]="order()"
          (columnOrderChange)="reorder($event)"
          [selectable]="selectable()"
          [selection]="selection()"
          (selectionChange)="selection.set($event)"
          [rowActions]="rowActions()"
          [clickable]="clickable()"
          [reload]="reload"
          [emptyTitle]="emptyTitle()"
          [emptyIcon]="emptyIcon()"
          [emptyActionLabel]="emptyActionLabel()"
          (rowClick)="onRowClick($event)"
          (rowAction)="onRowAction($event)"
          (cellChange)="onCellChange($event)"
          (linkClick)="onLink($event)"
          (emptyAction)="runEmpty()"
          (loaded)="remember($event)"
        >
          @for (toggle of readonlyToggles(); track toggle.key) {
            <ng-template [phxTableCell]="toggle.key" let-value>
              <phx-toggle [checked]="!!value" disabled [ariaLabel]="toggle.label" />
            </ng-template>
          }
          <span phxTableStats class="totals">
            @for (total of totals(); track total.key) {
              <span class="total" [attr.data-total]="total.key"
                >{{ total.label }}: {{ total.value }}</span
              >
            }
          </span>
        </phx-data-table>
      }
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
      min-height: 0;
    }
    .totals {
      display: contents;
    }
    .total {
      padding: 4px 10px;
      font: var(--phx-text-12-single);
      color: var(--phx-ink-muted);
      background-color: var(--phx-tonal);
      border-radius: var(--phx-radius-full);
    }
  `,
  host: {
    '[attr.data-list]': 'descriptor().name',
    '[attr.data-compact]': 'compact()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelListComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});
  readonly compact = input<number | null>(null);

  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly dates = inject(KeelDates);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly events = inject(KeelEvents);
  private readonly runner = inject(KeelActionRunner);
  private readonly navigation = inject(KeelNavigation);
  private readonly toasts = inject(PhxToastService);
  private readonly modals = inject(PhxModalService);
  private readonly injector = inject(Injector);
  private readonly exports = inject(KeelExports);
  private readonly orders = inject(KeelColumnOrders);
  private readonly remotes = signal<ReadonlyMap<string, RemoteOptions>>(new Map());
  private readonly refreshes = new Subject<void>();
  private readonly known = new Map<RowKey, Row>();

  readonly selection = signal<Set<RowKey>>(new Set());
  readonly reload = merge(
    this.refreshes,
    invalidations(this.events.dataInvalidate$, () => this.descriptor().ui?.invalidate_on ?? []),
  );

  readonly key = computed(() => keyOf(this.descriptor()));
  readonly title = computed(() => {
    const descriptor = this.descriptor();
    return descriptor.ui ? this.i18n.t(descriptor.ui.title) : descriptor.name;
  });
  readonly scope = computed(() => scopeOf(this.params()));
  readonly search = computed(() => searchOf(this.params()));
  readonly tableSearch = linkedSignal(() => this.search());
  readonly tableSort = signal<PhxSort | null>(null);
  readonly tableFilters = signal<PhxColumnFilter[]>([]);
  readonly order = signal<string[] | null>(null);
  readonly exporting = signal(false);
  readonly fetch = computed<KeelListFetch<Row>>(() =>
    listFetch<Row>(this.descriptor().name, this.scope(), this.client),
  );
  readonly compactRows = toSignal(
    combineLatest([toObservable(this.fetch), toObservable(this.compact)]).pipe(
      switchMap(([fetch, limit]) =>
        limit === null
          ? of(COMPACT_LOADING)
          : this.reload.pipe(
              startWith(null),
              switchMap(() => this.firstRows(fetch, limit)),
              startWith(COMPACT_LOADING),
            ),
      ),
    ),
    { initialValue: COMPACT_LOADING },
  );
  readonly exportable = toSignal(
    toObservable(this.descriptor).pipe(
      switchMap((descriptor) =>
        toolbarActionsOf(descriptor).some((action) => action.role === 'export')
          ? from(this.descriptors.list()).pipe(
              map((index) => index.some((entry) => entry.name === EXPORT_FUNCTION)),
              catchError(() => of(false)),
            )
          : of(false),
      ),
    ),
    { initialValue: false },
  );
  readonly multiForm = toSignal(
    toObservable(this.descriptor).pipe(
      switchMap((descriptor) => {
        const screen = editActionOf(descriptor)?.screen ?? null;
        return screen === null
          ? of<Descriptor | null>(null)
          : from(this.descriptors.get(screen)).pipe(
              map((form) => (isMultiForm(form) ? form : null)),
              catchError(() => of<Descriptor | null>(null)),
            );
      }),
    ),
    { initialValue: null },
  );
  readonly columns = computed<PhxColumn<Row>[]>(() => {
    const remotes = this.remotes();
    return columnsOf(this.descriptor(), {
      messages: this.i18n.messages(),
      zone: this.dates.zone(),
      locale: this.dates.locale(),
      choices: (func) => remotes.get(func)?.choices() ?? null,
    });
  });
  readonly totals = computed<ColumnTotal[]>(() =>
    totalsOf(this.descriptor(), this.fetch().totals(), {
      messages: this.i18n.messages(),
      locale: this.dates.locale(),
    }),
  );
  readonly readonlyToggles = computed<ReadonlyToggle[]>(() =>
    this.columns()
      .filter((column) => column.type === 'toggle' && this.editOf(column.key) === null)
      .map((column) => ({ key: column.key, label: column.label })),
  );
  readonly searchable = computed(() => (this.descriptor().ui?.search ?? []).length > 0);
  readonly toolbar = computed(() =>
    toolbarActionsOf(this.descriptor()).filter(
      (action) => action.role !== 'export' || this.exportable(),
    ),
  );
  readonly bulkActions = computed(() => bulkActionsOf(this.descriptor()));
  readonly multiEdit = computed(() =>
    this.multiForm() === null ? null : editActionOf(this.descriptor()),
  );
  readonly selectable = computed(() => this.bulkActions().length > 0 || this.multiEdit() !== null);
  readonly bulk = computed<PhxSelectionAction[]>(() => {
    const edit = this.multiEdit();
    const actions = edit === null ? this.bulkActions() : [edit, ...this.bulkActions()];
    return actions.map((action) => {
      const item: PhxSelectionAction = {
        id: action.id,
        label: this.labelOf(action),
        appearance: appearanceOf(action),
      };
      const icon = iconOf(action);
      if (icon !== null) {
        item.icon = icon;
      }
      return item;
    });
  });
  readonly rowActions = computed<PhxRowAction<Row>[]>(() => {
    const scope = this.rowScope();
    return rowActionsOf(this.descriptor()).map((action) => ({
      id: action.id,
      label: this.labelOf(action),
      appearance: action.role === 'delete' ? 'danger' : 'default',
      hidden: (row: Row) => isHidden(action, row, scope),
    }));
  });
  readonly defaultAction = computed(() => defaultActionOf(this.descriptor()));
  readonly clickable = computed(() => this.defaultAction() !== null);
  readonly emptyAction = computed(() => emptyActionOf(this.descriptor()));
  readonly emptyTitle = computed(() => {
    const empty = this.descriptor().ui?.empty ?? null;
    return empty === null ? null : this.i18n.t(empty.title);
  });
  readonly emptyActionLabel = computed(() => {
    const action = this.emptyAction();
    return action === null ? null : this.labelOf(action);
  });
  readonly emptyIcon = computed<PhxIconName>(() => {
    const action = this.emptyAction();
    return (action === null ? null : iconOf(action)) ?? DEFAULT_EMPTY_ICON;
  });

  private readonly rowScope = computed<RowScope>(() => {
    const principal = this.boot.principal();
    return {
      principal: principal === null ? null : { level: principal.level },
      params: this.scope(),
    };
  });

  constructor() {
    effect((onCleanup) => {
      const descriptor = this.descriptor();
      const scope = this.scope();
      const opened = untracked(() => this.openRemotes(descriptor, scope));
      this.remotes.set(opened);
      onCleanup(() => {
        for (const remote of opened.values()) {
          remote.destroy();
        }
      });
    });
    effect(() => {
      this.fetch();
      untracked(() => {
        this.known.clear();
        this.clearSelection();
      });
    });
    effect(() => {
      const name = this.descriptor().name;
      untracked(() => {
        this.tableSort.set(null);
        this.tableFilters.set([]);
        void this.loadOrder(name);
      });
    });
  }

  protected formatsOf(action: Action): readonly ExportFormat[] {
    return formatsOf(action);
  }

  protected formatLabel(format: ExportFormat): string {
    return this.i18n.t(formatKeyOf(format));
  }

  visibleColumns(): string[] {
    const declared = this.columns().map((column) => column.key);
    const order = this.order();
    return order === null ? declared : reordered(declared, order);
  }

  async export(action: Action, format: ExportFormat): Promise<void> {
    if (this.exporting()) {
      return;
    }
    this.exporting.set(true);
    try {
      await this.exports.run(
        {
          func: this.descriptor().name,
          query: exportQueryOf(
            { search: this.tableSearch(), sort: this.tableSort(), filters: this.tableFilters() },
            this.scope(),
          ),
          columns: this.visibleColumns(),
          format,
        },
        this.labelOf(action),
      );
    } finally {
      this.exporting.set(false);
    }
  }

  protected reorder(order: string[] | null): void {
    this.order.set(order);
    if (order !== null) {
      void this.orders.save(this.descriptor().name, order);
    }
  }

  private async loadOrder(name: string): Promise<void> {
    const stored = await this.orders.load(name);
    if (this.descriptor().name === name) {
      this.order.set(stored);
    }
  }

  labelOf(action: Action): string {
    return this.i18n.t(labelKeyOf(this.descriptor(), action));
  }

  protected iconOf(action: Action): PhxIconName | null {
    return iconOf(action);
  }

  protected appearanceOf(action: Action): PhxButtonAppearance {
    return appearanceOf(action);
  }

  refresh(): void {
    this.refreshes.next();
  }

  protected firstRowsOf(state: CompactRows): Row[] {
    return state.phase === 'loaded' ? state.rows : [];
  }

  private firstRows(fetch: KeelListFetch<Row>, limit: number): Observable<CompactRows> {
    return fetch({
      page: 1,
      pageSize: limit,
      offset: 0,
      limit,
      search: '',
      sort: null,
      filters: [],
    }).pipe(
      map((page): CompactRows => ({ phase: 'loaded', rows: page.data, total: page.total })),
      catchError(() => of<CompactRows>({ phase: 'loaded', rows: [], total: 0 })),
    );
  }

  clearSelection(): void {
    if (this.selection().size > 0) {
      this.selection.set(new Set());
    }
  }

  selectedRows(): Row[] {
    const rows: Row[] = [];
    for (const key of this.selection()) {
      const row = this.known.get(key);
      if (row !== undefined) {
        rows.push(row);
      }
    }
    return rows;
  }

  run(action: Action, rows: readonly Row[] = []): void {
    if (action.role === 'refresh') {
      this.refresh();
      return;
    }
    const screen = action.screen ?? null;
    if (screen !== null) {
      void this.navigation.screen({
        screen,
        params: screenParamsOf(this.descriptor(), action, rows[0]),
      });
      return;
    }
    if (action.function) {
      void this.runner.run({ descriptor: this.descriptor(), action, rows }).then((outcome) => {
        if (outcome?.ok) {
          this.clearSelection();
          this.refresh();
        }
      });
    }
  }

  protected runBulk(item: PhxSelectionAction): void {
    const action = actionOf(this.descriptor(), item.id);
    if (action === null) {
      return;
    }
    if (action === this.multiEdit()) {
      void this.editSelected();
      return;
    }
    this.run(action, this.selectedRows());
  }

  async editSelected(): Promise<void> {
    const form = this.multiForm();
    if (form === null) {
      return;
    }
    const ids = idsOf(this.descriptor(), this.selectedRows());
    const outcome = await openMultiForm(this.modals, this.injector, {
      descriptor: form,
      ids,
      title: this.i18n.t(MULTI_TITLE_KEY, { count: ids.length }),
    });
    if (outcome !== undefined) {
      this.clearSelection();
      this.refresh();
    }
  }

  protected runEmpty(): void {
    const action = this.emptyAction();
    if (action !== null) {
      this.run(action);
    }
  }

  protected onRowClick(event: PhxRowEvent<Row>): void {
    const action = this.defaultAction();
    if (action !== null && !isHidden(action, event.row, this.rowScope())) {
      this.run(action, [event.row]);
    }
  }

  protected onRowAction(event: PhxRowActionEvent<Row>): void {
    const action = actionOf(this.descriptor(), event.action.id);
    if (action !== null) {
      this.run(action, [event.row]);
    }
  }

  protected onLink(change: PhxCellChange<Row>): void {
    const to = this.columnOf(change.column.key)?.to ?? null;
    if (to === null) {
      return;
    }
    const id = String(change.row[this.key()]);
    void this.navigation.object(
      { screen: to.screen, params: substitute(to.params ?? {}, id) },
      String(change.value ?? id),
    );
  }

  protected onCellChange(change: PhxCellChange<Row>): void {
    const edit = this.editOf(change.column.key);
    if (edit === null) {
      return;
    }
    const body = { [this.key()]: change.row[this.key()], [change.column.key]: change.value };
    void firstValueFrom(
      this.client.call(edit.function, body, { idempotencyKey: newIdempotencyKey() }),
    )
      .catch((error: unknown): Envelope => ({ ok: false, error: unreachable(error) }))
      .then((envelope) => {
        if (!envelope.ok) {
          this.toasts.danger(describeError(this.i18n, envelope.error), {
            title: change.column.label,
          });
        }
        this.refresh();
      });
  }

  protected remember(page: PhxDataTablePage<Row>): void {
    const key = this.key();
    for (const row of page.data) {
      const value = row[key];
      this.known.set(typeof value === 'number' ? value : String(value), row);
    }
  }

  private columnOf(key: string): Column | null {
    return (this.descriptor().ui?.columns ?? []).find((column) => column.key === key) ?? null;
  }

  private editOf(key: string): InlineEdit | null {
    return this.columnOf(key)?.edit ?? null;
  }

  private openRemotes(
    descriptor: Descriptor,
    params: Record<string, unknown>,
  ): ReadonlyMap<string, RemoteOptions> {
    const opened = new Map<string, RemoteOptions>();
    for (const column of descriptor.ui?.columns ?? []) {
      const filter = column.filter ?? null;
      if (filter === null || !('remote' in filter) || opened.has(filter.remote)) {
        continue;
      }
      const remote = new RemoteOptions(this.client, filter.remote, params);
      remote.search('');
      opened.set(filter.remote, remote);
    }
    return opened;
  }
}

function editActionOf(descriptor: Descriptor): Action | null {
  return (
    (descriptor.ui?.actions ?? []).find(
      (action) => action.role === 'edit' && (action.screen ?? null) !== null,
    ) ?? null
  );
}
