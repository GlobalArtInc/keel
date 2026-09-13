import { NgComponentOutlet } from '@angular/common';
import {
  ChangeDetectionStrategy,
  Component,
  Injector,
  computed,
  inject,
  input,
  output,
} from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { ActivatedRoute, type ParamMap } from '@angular/router';
import {
  KeelBoot,
  KeelCallError,
  KeelClient,
  KeelDescriptors,
  KeelI18n,
} from '@globalart/keel/client';
import { isObject, unreachable, type Saved } from '@globalart/keel/forms';
import type { CallError, Descriptor } from '@globalart/keel/meta';
import {
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxNotFoundComponent,
  PhxPanelComponent,
  PhxTabsService,
} from '@globalart/phoenix-ui';
import {
  catchError,
  combineLatest,
  from,
  map,
  of,
  startWith,
  switchMap,
  type Observable,
} from 'rxjs';
import { KeelActionComponent } from './action/action.component';
import { KeelFormScreenComponent } from './form/form-screen.component';
import { KeelObjectComponent } from './object/object.component';
import { NOT_FOUND_KEY, SCREEN_UNSUPPORTED_KEY, VIEW_MISSING_KEY, describeError } from './messages';
import {
  injectKeelViews,
  productOf,
  viewInjector,
  viewOf,
  type KeelView,
  type KeelViewContext,
} from './views';

export type ScreenState =
  | { phase: 'loading' }
  | { phase: 'view'; view: KeelView; injector: Injector }
  | { phase: 'screen'; descriptor: Descriptor }
  | { phase: 'missing'; product: string }
  | { phase: 'failed'; error: CallError };

const LOADING: ScreenState = { phase: 'loading' };
const NO_PARAMS: Record<string, unknown> = {};

export function paramsOf(query: ParamMap): Record<string, unknown> {
  const params: Record<string, unknown> = {};
  for (const key of query.keys) {
    const values = query.getAll(key);
    params[key] = values.length === 1 ? values[0] : values;
  }
  return params;
}

@Component({
  selector: 'keel-screen',
  imports: [
    NgComponentOutlet,
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxNotFoundComponent,
    KeelFormScreenComponent,
    KeelObjectComponent,
    KeelActionComponent,
  ],
  template: `
    @switch (state().phase) {
      @case ('loading') {
        <phx-panel
          ><div class="loading"><phx-loader [size]="24" /></div
        ></phx-panel>
      }
      @case ('view') {
        <ng-container *ngComponentOutlet="view().view.component; injector: view().injector" />
      }
      @case ('missing') {
        <phx-not-found [title]="notFoundTitle()" [text]="missingText()" [actionLabel]="null" />
      }
      @case ('failed') {
        @if (failure().code === 'not_found') {
          <phx-not-found [title]="notFoundTitle()" [text]="message()" [actionLabel]="null" />
        } @else {
          <phx-panel><phx-error [text]="described()" /></phx-panel>
        }
      }
      @case ('screen') {
        @switch (descriptor().kind) {
          @case ('form') {
            <keel-form-screen
              [descriptor]="descriptor()"
              [params]="screenParams()"
              (saved)="done.emit($event)"
              (closed)="closed.emit()"
            />
          }
          @case ('get') {
            <keel-object [descriptor]="descriptor()" [params]="screenParams()" />
          }
          @case ('action') {
            <keel-action
              [descriptor]="descriptor()"
              [params]="screenParams()"
              (done)="done.emit($event)"
              (closed)="closed.emit()"
            />
          }
          @default {
            <phx-not-found
              [title]="notFoundTitle()"
              [text]="unsupportedText()"
              [actionLabel]="null"
            />
          }
        }
      }
    }
  `,
  styles: `
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
  `,
  host: {
    '[attr.data-screen]': 'func()',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelScreenComponent {
  readonly func = input.required<string>();
  readonly params = input<Record<string, unknown> | null>(null);

  readonly done = output<Saved>();
  readonly closed = output<void>();

  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly i18n = inject(KeelI18n);
  private readonly tabs = inject(PhxTabsService);
  private readonly injector = inject(Injector);
  private readonly views = injectKeelViews();
  private readonly route = inject(ActivatedRoute, { optional: true });
  private readonly query = this.route
    ? toSignal(this.route.queryParamMap.pipe(map(paramsOf)), { initialValue: NO_PARAMS })
    : computed(() => NO_PARAMS);

  readonly screenParams = computed(() => {
    const given = this.params();
    return isObject(given) ? given : this.query();
  });

  readonly state = toSignal(
    combineLatest([toObservable(this.func), toObservable(this.screenParams)]).pipe(
      switchMap(([func, params]) => this.resolve(func, params).pipe(startWith(LOADING))),
    ),
    { initialValue: LOADING },
  );

  protected readonly view = computed(() => {
    const state = this.state();
    if (state.phase !== 'view') {
      throw new TypeError(`the screen ${this.func()} is not a view`);
    }
    return state;
  });
  protected readonly descriptor = computed(() => {
    const state = this.state();
    if (state.phase !== 'screen') {
      throw new TypeError(`the screen ${this.func()} has no descriptor`);
    }
    return state.descriptor;
  });
  protected readonly failure = computed(() => {
    const state = this.state();
    if (state.phase !== 'failed') {
      throw new TypeError(`the screen ${this.func()} did not fail`);
    }
    return state.error;
  });
  protected readonly notFoundTitle = computed(() => this.i18n.t(NOT_FOUND_KEY));
  protected readonly message = computed(() => {
    const error = this.failure();
    return this.i18n.t(error.key, error.params);
  });
  protected readonly described = computed(() => describeError(this.i18n, this.failure()));
  protected readonly missingText = computed(() => {
    const state = this.state();
    const product = state.phase === 'missing' ? state.product : '';
    const named = this.boot.products().find((entry) => entry.id === product);
    return this.i18n.t(VIEW_MISSING_KEY, {
      view: this.func(),
      product: named === undefined ? product : this.i18n.t(named.name),
    });
  });
  protected readonly unsupportedText = computed(() =>
    this.i18n.t(SCREEN_UNSUPPORTED_KEY, { kind: this.descriptor().kind }),
  );

  private resolve(func: string, params: Record<string, unknown>): Observable<ScreenState> {
    const view = viewOf(this.views, func);
    if (this.boot.views().includes(func)) {
      return of<ScreenState>(
        view === undefined
          ? { phase: 'missing', product: productOf(this.boot.payload(), func) }
          : this.mount(view, params),
      );
    }
    return from(this.descriptors.get(func)).pipe(
      map((descriptor): ScreenState =>
        view === undefined ? { phase: 'screen', descriptor } : this.mount(view, params, descriptor),
      ),
      catchError((error: unknown) =>
        of<ScreenState>({
          phase: 'failed',
          error: error instanceof KeelCallError ? error.error : unreachable(error),
        }),
      ),
    );
  }

  private mount(
    view: KeelView,
    params: Record<string, unknown>,
    descriptor?: Descriptor,
  ): ScreenState {
    const context: KeelViewContext = {
      params,
      client: this.client,
      principal: this.boot.principal(),
      tabs: this.tabs,
    };
    if (descriptor !== undefined) {
      context.descriptor = descriptor;
    }
    return { phase: 'view', view, injector: viewInjector(context, this.injector) };
  }
}
