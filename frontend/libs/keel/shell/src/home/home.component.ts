import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import { KeelBoot, KeelI18n } from '@globalart/keel/client';
import { lookup, type BootWidget } from '@globalart/keel/meta';
import { KeelListComponent, SCREEN_PATH, describeError } from '@globalart/keel/renderer';
import {
  PhxButtonComponent,
  PhxDropdownComponent,
  PhxDropdownItemComponent,
  PhxDropdownTriggerDirective,
  PhxIconButtonComponent,
  PhxStatComponent,
  PhxTextButtonComponent,
  PhxWidgetCardComponent,
  PhxWidgetGridComponent,
  type PhxWidgetReorderEvent,
} from '@globalart/phoenix-ui';
import { KeelNotFoundComponent } from '../not-found';
import { plainClick } from '../layout/layout.component';
import { KeelDashboard, reordered, withHidden, type WidgetState } from './dashboard';
import { KeelSparklineComponent } from './sparkline.component';

export const HOME_OPEN_KEY = 'keel.shell.home.open';
export const HOME_TITLE_KEY = 'keel.shell.home.title';
const HOME_HIDE_KEY = 'keel.shell.home.hide';
const HOME_RESTORE_KEY = 'keel.shell.home.restore';
const HOME_EMPTY_KEY = 'keel.shell.home.empty';
const HOME_DRAG_KEY = 'keel.shell.home.drag';
const HOME_SPARKLINE_KEY = 'keel.shell.home.sparkline';
const HOME_UNIT_PREFIX = 'keel.shell.home.unit.';
export const DEFAULT_WIDGET_LIMIT = 5;
export const HOME_COLUMNS = 2;

export function limitOf(widget: BootWidget): number {
  const limit = widget.limit ?? null;
  return limit === null || limit <= 0 ? DEFAULT_WIDGET_LIMIT : limit;
}

@Component({
  selector: 'keel-home',
  imports: [
    PhxWidgetGridComponent,
    PhxWidgetCardComponent,
    PhxTextButtonComponent,
    PhxButtonComponent,
    PhxIconButtonComponent,
    PhxDropdownComponent,
    PhxDropdownItemComponent,
    PhxDropdownTriggerDirective,
    PhxStatComponent,
    KeelListComponent,
    KeelNotFoundComponent,
    KeelSparklineComponent,
  ],
  template: `
    @if (widgets().length === 0) {
      <keel-not-found />
    } @else {
      <div class="head">
        <h1 class="title">{{ title() }}</h1>
        @if (hidden().length > 0) {
          <button
            phxButton="default"
            type="button"
            icon="eye"
            data-action="restore"
            [phxDropdownTrigger]="restoreMenu"
          >
            {{ restoreLabel() }}
          </button>
          <phx-dropdown #restoreMenu menu align="end" [label]="restoreLabel()">
            @for (widget of hidden(); track widget.func) {
              <phx-dropdown-item
                [attr.data-widget-func]="widget.func"
                (select)="restoreMenu.select(); restore(widget)"
              >
                {{ titleOf(stateOf(widget)) }}
              </phx-dropdown-item>
            }
          </phx-dropdown>
        }
      </div>
      @if (visible().length === 0) {
        <p class="empty">{{ emptyLabel() }}</p>
      } @else {
        <phx-widget-grid [columns]="columns" [order]="order()" (reorder)="reorder($event)">
          @for (widget of visible(); track widget.func) {
            @let state = stateOf(widget);
            <phx-widget-card
              [id]="widget.func"
              [title]="titleOf(state)"
              [loading]="state.phase === 'loading'"
              [error]="errorOf(state)"
              [dragLabel]="dragLabel()"
              [attr.data-widget-func]="widget.func"
              [attr.data-widget-kind]="widget.kind"
            >
              @if (widget.kind === 'list') {
                <a
                  phxWidgetActions
                  phxTextButton="link"
                  class="open"
                  [href]="urlOf(widget)"
                  (click)="open($event, widget)"
                >
                  {{ openLabel() }}
                </a>
              }
              <button
                phxWidgetActions
                phxIconButton
                type="button"
                icon="eye-off"
                data-action="hide"
                [label]="hideLabel()"
                [title]="hideLabel()"
                (click)="hide(widget)"
              ></button>
              @if (state.phase === 'ready') {
                @if (state.stat; as stat) {
                  <phx-stat
                    [panel]="false"
                    [label]="unitOf(stat.unit)"
                    [value]="stat.value"
                    [locale]="locale()"
                  />
                  @if (stat.points.length > 1) {
                    <keel-sparkline [points]="stat.points" [label]="sparklineLabel()" />
                  }
                } @else {
                  <keel-list [descriptor]="state.descriptor" [compact]="limitOf(widget)" />
                }
              }
            </phx-widget-card>
          }
        </phx-widget-grid>
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
    .head {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      align-items: center;
      justify-content: space-between;
    }
    .title {
      margin: 0;
      font: var(--phx-heading-25);
      color: var(--phx-ink);
    }
    .empty {
      margin: 0;
      font: var(--phx-text-14);
      color: var(--phx-ink-muted);
    }
    keel-sparkline {
      margin-top: 8px;
    }
  `,
  host: {
    '[attr.data-widgets]': 'visible().length',
    '[attr.data-order]': 'order().join(" ")',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelHomeComponent {
  private readonly boot = inject(KeelBoot);
  private readonly dashboard = inject(KeelDashboard);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);

  protected readonly columns = HOME_COLUMNS;
  readonly widgets = signal<readonly BootWidget[]>(this.boot.payload()?.dashboard ?? []);
  readonly visible = computed(() => this.widgets().filter((widget) => widget.hidden !== true));
  readonly hidden = computed(() => this.widgets().filter((widget) => widget.hidden === true));
  protected readonly order = computed(() => this.visible().map((widget) => widget.func));
  private readonly states = signal<ReadonlyMap<string, WidgetState>>(new Map());

  protected readonly title = computed(() => this.i18n.t(HOME_TITLE_KEY));
  protected readonly openLabel = computed(() => this.i18n.t(HOME_OPEN_KEY));
  protected readonly hideLabel = computed(() => this.i18n.t(HOME_HIDE_KEY));
  protected readonly restoreLabel = computed(() => this.i18n.t(HOME_RESTORE_KEY));
  protected readonly emptyLabel = computed(() => this.i18n.t(HOME_EMPTY_KEY));
  protected readonly dragLabel = computed(() => this.i18n.t(HOME_DRAG_KEY));
  protected readonly sparklineLabel = computed(() => this.i18n.t(HOME_SPARKLINE_KEY));
  protected readonly locale = computed(() => this.i18n.lang() ?? undefined);

  constructor() {
    for (const widget of this.widgets()) {
      void this.dashboard.load(widget).then((state) => this.settle(state));
    }
  }

  protected stateOf(widget: BootWidget): WidgetState {
    return this.states().get(widget.func) ?? { phase: 'loading', widget };
  }

  protected limitOf(widget: BootWidget): number {
    return limitOf(widget);
  }

  protected titleOf(state: WidgetState): string {
    return state.phase === 'ready'
      ? this.i18n.t(state.descriptor.ui?.title ?? state.descriptor.name)
      : state.widget.func;
  }

  protected errorOf(state: WidgetState): string | null {
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  }

  protected unitOf(unit: string): string {
    return lookup(this.i18n.messages(), `${HOME_UNIT_PREFIX}${unit}`) ?? unit;
  }

  protected urlOf(widget: BootWidget): string {
    return `/${SCREEN_PATH}/${widget.func}`;
  }

  protected open(event: MouseEvent, widget: BootWidget): void {
    if (!plainClick(event)) {
      return;
    }
    event.preventDefault();
    void this.router.navigateByUrl(this.urlOf(widget));
  }

  protected reorder(event: PhxWidgetReorderEvent): void {
    this.change(reordered(this.widgets(), event.order));
  }

  protected hide(widget: BootWidget): void {
    this.change(withHidden(this.widgets(), widget.func, true));
  }

  protected restore(widget: BootWidget): void {
    this.change(withHidden(this.widgets(), widget.func, false));
  }

  private change(widgets: BootWidget[]): void {
    this.widgets.set(widgets);
    void this.dashboard.save(widgets);
  }

  private settle(state: WidgetState): void {
    this.states.update((states) => new Map(states).set(state.widget.func, state));
  }
}
