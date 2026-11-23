import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { KeelBoot, KeelCallError, KeelClient, KeelEvents, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Action, CallError, Descriptor, Page } from '@globalart/keel/meta';
import {
  PhxButtonComponent,
  PhxEmptyComponent,
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxPageHeaderComponent,
  PhxPanelComponent,
  PhxSelectionBarComponent,
  PhxToastService,
  PhxTreeComponent,
  type PhxButtonAppearance,
  type PhxIconName,
  type PhxSelectionAction,
  type PhxTreeLoadChildren,
  type PhxTreeNode,
} from '@globalart/phoenix-ui';
import {
  Subject,
  catchError,
  combineLatest,
  firstValueFrom,
  map,
  merge,
  of,
  startWith,
  switchMap,
  type Observable,
} from 'rxjs';
import { KeelActionRunner, labelKeyOf, type Row } from '../action/runner';
import {
  actionOf,
  appearanceOf,
  iconOf,
  isHidden,
  rowActionsOf,
  screenParamsOf,
  toolbarActionsOf,
  type RowScope,
} from '../list/actions';
import { invalidations } from '../list/invalidate';
import { describeError } from '../messages';
import { KeelNavigation } from '../navigation';
import { KEEL_SCREEN_DEPTH } from '../depth';
import { KEEL_WORKSPACE } from '../workspace';
import { levelQuery, nodesOf, parentOf, rowOf } from './tree';

type TreeRoots =
  | { phase: 'loading' }
  | { phase: 'loaded'; nodes: PhxTreeNode[] }
  | { phase: 'failed'; error: CallError };

const LOADING: TreeRoots = { phase: 'loading' };
const EMPTY_ICON: PhxIconName = 'folder';

@Component({
  selector: 'keel-tree',
  imports: [
    PhxPageHeaderComponent,
    PhxButtonComponent,
    PhxSelectionBarComponent,
    PhxTreeComponent,
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxEmptyComponent,
  ],
  template: `
    <phx-page-header [title]="title()" [attached]="depth === 0 && (workspace?.visible() ?? false)">
      @for (action of toolbar(); track action.id) {
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
    </phx-page-header>
    <phx-selection-bar
      [count]="chosen() === null ? 0 : 1"
      [label]="chosen()?.label ?? null"
      [actions]="chosenActions()"
      (action)="runOnChosen($event)"
      (clear)="choose(null)"
    />
    @switch (roots().phase) {
      @case ('loading') {
        <phx-panel
          ><div class="loading"><phx-loader [size]="24" /></div
        ></phx-panel>
      }
      @case ('failed') {
        <phx-panel><phx-error [text]="failure()" /></phx-panel>
      }
      @case ('loaded') {
        @if (nodes().length === 0) {
          <phx-empty [title]="emptyTitle()" [icon]="emptyIcon" />
        } @else {
          <phx-tree
            [nodes]="nodes()"
            [loadChildren]="children"
            [label]="title()"
            [selected]="chosen()?.id ?? null"
            (nodeSelect)="choose($event)"
            (loadError)="refused($event.error)"
          />
        }
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
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
  `,
  host: {
    '[attr.data-tree]': 'descriptor().name',
    '[attr.data-phase]': 'roots().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelTreeComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});

  protected readonly depth = inject(KEEL_SCREEN_DEPTH, { optional: true }) ?? -1;
  protected readonly workspace = inject(KEEL_WORKSPACE, { optional: true });
  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly events = inject(KeelEvents);
  private readonly runner = inject(KeelActionRunner);
  private readonly navigation = inject(KeelNavigation);
  private readonly toasts = inject(PhxToastService);
  private readonly refreshes = new Subject<void>();

  protected readonly emptyIcon = EMPTY_ICON;
  readonly chosen = signal<PhxTreeNode | null>(null);
  readonly roots = toSignal(
    combineLatest([
      toObservable(this.descriptor),
      toObservable(this.params),
      merge(
        this.refreshes,
        invalidations(this.events.dataInvalidate$, () => this.descriptor().ui?.invalidate_on ?? []),
      ).pipe(startWith(null)),
    ]).pipe(
      switchMap(([descriptor, params]) =>
        this.level(descriptor, params, null).pipe(
          map((rows): TreeRoots => ({ phase: 'loaded', nodes: nodesOf(descriptor, rows) })),
          catchError((error: unknown) =>
            of<TreeRoots>({
              phase: 'failed',
              error: error instanceof KeelCallError ? error.error : unreachable(error),
            }),
          ),
          startWith(LOADING),
        ),
      ),
    ),
    { initialValue: LOADING },
  );

  readonly title = computed(() => {
    const descriptor = this.descriptor();
    return descriptor.ui ? this.i18n.t(descriptor.ui.title) : descriptor.name;
  });
  readonly nodes = computed(() => {
    const roots = this.roots();
    return roots.phase === 'loaded' ? roots.nodes : [];
  });
  readonly toolbar = computed(() =>
    toolbarActionsOf(this.descriptor()).filter((action) => action.role !== 'export'),
  );
  readonly chosenActions = computed<PhxSelectionAction[]>(() => {
    const node = this.chosen();
    if (node === null) {
      return [];
    }
    const row = rowOf(node);
    const scope = this.rowScope();
    return rowActionsOf(this.descriptor())
      .filter((action) => !isHidden(action, row, scope))
      .map((action) => {
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
  protected readonly failure = computed(() => {
    const roots = this.roots();
    return roots.phase === 'failed' ? describeError(this.i18n, roots.error) : '';
  });
  protected readonly emptyTitle = computed(() => {
    const empty = this.descriptor().ui?.empty ?? null;
    return empty === null ? null : this.i18n.t(empty.title);
  });

  readonly children: PhxTreeLoadChildren = (node) => {
    const descriptor = this.descriptor();
    return firstValueFrom(this.level(descriptor, this.params(), node.id)).then((rows) =>
      nodesOf(descriptor, rows),
    );
  };

  private readonly rowScope = computed<RowScope>(() => {
    const principal = this.boot.principal();
    return {
      principal: principal === null ? null : { level: principal.level },
      params: this.params(),
    };
  });

  labelOf(action: Action): string {
    return this.i18n.t(labelKeyOf(this.descriptor(), action));
  }

  protected iconOf(action: Action): PhxIconName | null {
    return iconOf(action);
  }

  protected appearanceOf(action: Action): PhxButtonAppearance {
    return appearanceOf(action);
  }

  choose(node: PhxTreeNode | null): void {
    this.chosen.set(node);
  }

  refresh(): void {
    this.chosen.set(null);
    this.refreshes.next();
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
        params: screenParamsOf(this.descriptor(), action, this.params(), rows[0]),
      });
      return;
    }
    if (action.function) {
      void this.runner.run({ descriptor: this.descriptor(), action, rows }).then((outcome) => {
        if (outcome?.ok) {
          this.refresh();
        }
      });
    }
  }

  protected runOnChosen(item: PhxSelectionAction): void {
    const node = this.chosen();
    const action = actionOf(this.descriptor(), item.id);
    if (node !== null && action !== null) {
      this.run(action, [rowOf(node)]);
    }
  }

  protected refused(error: unknown): void {
    const failure = error instanceof KeelCallError ? error.error : unreachable(error);
    this.toasts.danger(describeError(this.i18n, failure), { title: this.title() });
  }

  private level(
    descriptor: Descriptor,
    params: Record<string, unknown>,
    under: string | null,
  ): Observable<Row[]> {
    const parent = parentOf(descriptor);
    if (parent === null) {
      return of([]);
    }
    return this.client.call<Page<Row>>(descriptor.name, levelQuery(parent, under, params)).pipe(
      map((envelope) => {
        if (!envelope.ok) {
          throw new KeelCallError(envelope.error);
        }
        return envelope.data.data;
      }),
    );
  }
}
