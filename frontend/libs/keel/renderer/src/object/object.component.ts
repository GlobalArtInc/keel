import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { CallError, Descriptor, Envelope } from '@globalart/keel/meta';
import { PhxErrorComponent, PhxLoaderComponent, PhxPanelComponent } from '@globalart/phoenix-ui';
import { catchError, combineLatest, map, of, startWith, switchMap } from 'rxjs';
import { RETRY_KEY, describeError } from '../messages';
import { KeelPropsComponent } from './props.component';

export type ObjectState =
  { phase: 'loading' } | { phase: 'loaded'; data: unknown } | { phase: 'failed'; error: CallError };

const LOADING: ObjectState = { phase: 'loading' };

@Component({
  selector: 'keel-object',
  imports: [PhxPanelComponent, PhxLoaderComponent, PhxErrorComponent, KeelPropsComponent],
  template: `
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

  retry(): void {
    this.attempt.update((count) => count + 1);
  }
}

function settle(envelope: Envelope): ObjectState {
  return envelope.ok
    ? { phase: 'loaded', data: envelope.data }
    : { phase: 'failed', error: envelope.error };
}
