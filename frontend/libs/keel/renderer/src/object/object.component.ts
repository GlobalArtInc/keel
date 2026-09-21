import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { isObject, unreachable } from '@globalart/keel/forms';
import type { Action, CallError, Descriptor, Envelope } from '@globalart/keel/meta';
import {
  PhxButtonComponent,
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxPageHeaderComponent,
  PhxPanelComponent,
} from '@globalart/phoenix-ui';
import { catchError, combineLatest, map, of, startWith, switchMap } from 'rxjs';
import { KeelActionRunner, labelKeyOf, type Row } from '../action/runner';
import {
  appearanceOf,
  iconOf,
  isHidden,
  screenParamsOf,
  toolbarActionsOf,
  type RowScope,
} from '../list/actions';
import { RETRY_KEY, describeError } from '../messages';
import { KeelNavigation } from '../navigation';
import { KeelPropsComponent } from './props.component';

export type ObjectState =
  { phase: 'loading' } | { phase: 'loaded'; data: unknown } | { phase: 'failed'; error: CallError };

const LOADING: ObjectState = { phase: 'loading' };
const NO_ROW: Row = {};

@Component({
  selector: 'keel-object',
  imports: [
    PhxPageHeaderComponent,
    PhxButtonComponent,
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    KeelPropsComponent,
  ],
  template: `
    @if (title(); as title) {
      <phx-page-header [title]="title">
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
    }
    <phx-panel>
      @switch (state().phase) {
        @case ('loading') {
          <div class="loading"><phx-loader [size]="24" /></div>
        }
        @case ('failed') {
          <phx-error [text]="message()" [retryLabel]="retryLabel()" (retry)="retry()" />
        }
        @case ('loaded') {
          <keel-props [descriptor]="descriptor()" [data]="data()" />
        }
      }
    </phx-panel>
  `,
  styles: `
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    :host {
      display: block;
    }
  `,
  host: {
    '[attr.data-object]': 'descriptor().name',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelObjectComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});

  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly boot = inject(KeelBoot);
  private readonly runner = inject(KeelActionRunner);
  private readonly navigation = inject(KeelNavigation);
  private readonly attempt = signal(0);

  readonly state = toSignal(
    combineLatest([
      toObservable(this.descriptor),
      toObservable(this.params),
      toObservable(this.attempt),
    ]).pipe(
      switchMap(([descriptor, params]) =>
        this.client.call(descriptor.name, params).pipe(
          map((envelope): ObjectState => settle(envelope)),
          catchError((error: unknown) =>
            of<ObjectState>({ phase: 'failed', error: unreachable(error) }),
          ),
          startWith(LOADING),
        ),
      ),
    ),
    { initialValue: LOADING },
  );

  protected readonly data = computed(() => {
    const state = this.state();
    return state.phase === 'loaded' ? state.data : null;
  });
  protected readonly message = computed(() => {
    const state = this.state();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly title = computed(() => {
    const title = this.descriptor().ui?.title ?? null;
    return title === null ? null : this.i18n.t(title);
  });
  protected readonly toolbar = computed(() => {
    const state = this.state();
    if (state.phase !== 'loaded') {
      return [];
    }
    const row = isObject(state.data) ? state.data : NO_ROW;
    const scope = this.scope();
    return toolbarActionsOf(this.descriptor()).filter((action) => !isHidden(action, row, scope));
  });

  private readonly scope = computed<RowScope>(() => {
    const principal = this.boot.principal();
    return {
      principal: principal === null ? null : { level: principal.level },
      params: this.params(),
    };
  });

  retry(): void {
    this.attempt.update((count) => count + 1);
  }

  run(action: Action): void {
    if (action.role === 'refresh') {
      this.retry();
      return;
    }
    const screen = action.screen ?? null;
    if (screen !== null) {
      void this.navigation.screen({
        screen,
        params: screenParamsOf(this.descriptor(), action),
      });
      return;
    }
    if (action.function) {
      void this.runner.run({ descriptor: this.descriptor(), action }).then((outcome) => {
        if (outcome?.ok) {
          this.retry();
        }
      });
    }
  }

  protected labelOf(action: Action): string {
    return this.i18n.t(labelKeyOf(this.descriptor(), action));
  }

  protected readonly appearanceOf = appearanceOf;
  protected readonly iconOf = iconOf;
}

function settle(envelope: Envelope): ObjectState {
  return envelope.ok
    ? { phase: 'loaded', data: envelope.data }
    : { phase: 'failed', error: envelope.error };
}
