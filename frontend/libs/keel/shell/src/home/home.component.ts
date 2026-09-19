import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { Router } from '@angular/router';
import { KeelBoot, KeelCallError, KeelDescriptors, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { BootWidget, CallError, Descriptor } from '@globalart/keel/meta';
import { KeelListComponent, SCREEN_PATH, describeError } from '@globalart/keel/renderer';
import {
  PhxTextButtonComponent,
  PhxWidgetCardComponent,
  PhxWidgetGridComponent,
} from '@globalart/phoenix-ui';
import { catchError, combineLatest, from, map, of, startWith, type Observable } from 'rxjs';
import { KeelNotFoundComponent } from '../not-found';
import { plainClick } from '../layout/layout.component';

export const HOME_OPEN_KEY = 'keel.shell.home.open';
export const HOME_TITLE_KEY = 'keel.shell.home.title';
export const DEFAULT_WIDGET_LIMIT = 5;
export const HOME_COLUMNS = 2;

export type WidgetState =
  | { phase: 'loading'; widget: BootWidget }
  | { phase: 'ready'; widget: BootWidget; descriptor: Descriptor }
  | { phase: 'failed'; widget: BootWidget; error: CallError };

export function widgetId(widget: BootWidget, index: number): string {
  return `${index}:${widget.kind}:${widget.func}`;
}

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
    KeelListComponent,
    KeelNotFoundComponent,
  ],
  template: `
    @if (widgets().length === 0) {
      <keel-not-found />
    } @else {
      <h1 class="title">{{ title() }}</h1>
      <phx-widget-grid [columns]="columns" locked>
        @for (state of states(); track $index; let index = $index) {
          <phx-widget-card
            [id]="idOf(state.widget, index)"
            [title]="titleOf(state)"
            [loading]="state.phase === 'loading'"
            [error]="errorOf(state)"
            [attr.data-widget-func]="state.widget.func"
          >
            <a
              phxWidgetActions
              phxTextButton="link"
              class="open"
              [href]="urlOf(state.widget)"
              (click)="open($event, state.widget)"
            >
              {{ openLabel() }}
            </a>
            @if (state.phase === 'ready') {
              <keel-list [descriptor]="state.descriptor" [compact]="limitOf(state.widget)" />
            }
          </phx-widget-card>
        }
      </phx-widget-grid>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
      min-height: 0;
    }
    .title {
      margin: 0;
      font: var(--phx-heading-25);
      color: var(--phx-ink);
    }
  `,
  host: {
    '[attr.data-widgets]': 'widgets().length',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelHomeComponent {
  private readonly boot = inject(KeelBoot);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);

  protected readonly columns = HOME_COLUMNS;
  readonly widgets = computed(() => this.boot.payload()?.dashboard ?? []);
  readonly states = toSignal(this.load(this.widgets()), { initialValue: [] as WidgetState[] });

  protected readonly title = computed(() => this.i18n.t(HOME_TITLE_KEY));
  protected readonly openLabel = computed(() => this.i18n.t(HOME_OPEN_KEY));

  protected idOf(widget: BootWidget, index: number): string {
    return widgetId(widget, index);
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

  private load(widgets: readonly BootWidget[]): Observable<WidgetState[]> {
    if (widgets.length === 0) {
      return of([]);
    }
    return combineLatest(
      widgets.map((widget) =>
        from(this.descriptors.get(widget.func)).pipe(
          map((descriptor): WidgetState => ({ phase: 'ready', widget, descriptor })),
          catchError((error: unknown) =>
            of<WidgetState>({
              phase: 'failed',
              widget,
              error: error instanceof KeelCallError ? error.error : unreachable(error),
            }),
          ),
          startWith<WidgetState>({ phase: 'loading', widget }),
        ),
      ),
    );
  }
}
