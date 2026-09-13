import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
} from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import type { Saved } from '@globalart/keel/forms';
import type { Action, CallError, Descriptor } from '@globalart/keel/meta';
import {
  PhxBannerComponent,
  PhxButtonComponent,
  PhxErrorComponent,
  PhxLoaderComponent,
  PhxPanelComponent,
} from '@globalart/phoenix-ui';
import { DONE_KEY, RETRY_KEY, describeError } from '../messages';
import { KeelPropsComponent } from '../object/props.component';
import { KeelActionRunner, type ActionRequest } from './runner';

export type ActionState =
  | { phase: 'running' }
  | { phase: 'refused' }
  | { phase: 'done'; data: unknown }
  | { phase: 'failed'; error: CallError };

export function routeAction(descriptor: Descriptor, params: Record<string, unknown>): Action {
  return {
    id: descriptor.name.slice(descriptor.name.lastIndexOf('.') + 1),
    role: 'button',
    function: descriptor.name,
    params,
    confirm: true,
  };
}

@Component({
  selector: 'keel-action',
  imports: [
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxBannerComponent,
    PhxButtonComponent,
    KeelPropsComponent,
  ],
  template: `
    <phx-panel [title]="label()">
      @switch (state().phase) {
        @case ('running') {
          <div class="loading"><phx-loader [size]="24" /></div>
        }
        @case ('refused') {
          <button phxButton="primary" type="button" class="run" (click)="run()">
            {{ label() }}
          </button>
        }
        @case ('failed') {
          <phx-error [text]="message()" [retryLabel]="retryLabel()" (retry)="run()" />
        }
        @case ('done') {
          <phx-banner class="done" status="success">{{ doneText() }}</phx-banner>
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
    .done {
      margin-bottom: 16px;
    }
  `,
  host: {
    '[attr.data-action]': 'descriptor().name',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelActionComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});

  readonly done = output<Saved>();
  readonly closed = output<void>();

  private readonly runner = inject(KeelActionRunner);
  private readonly i18n = inject(KeelI18n);
  private readonly held = signal<ActionState>({ phase: 'running' });
  private sequence = 0;

  readonly state = this.held.asReadonly();

  protected readonly request = computed<ActionRequest>(() => ({
    descriptor: this.descriptor(),
    action: routeAction(this.descriptor(), this.params()),
  }));
  protected readonly label = computed(() => this.runner.label(this.request()));
  protected readonly data = computed(() => {
    const state = this.state();
    return state.phase === 'done' ? state.data : null;
  });
  protected readonly message = computed(() => {
    const state = this.state();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly doneText = computed(() => this.i18n.t(DONE_KEY, { action: this.label() }));

  constructor() {
    effect(() => {
      this.request();
      untracked(() => this.run());
    });
  }

  run(): void {
    const started = ++this.sequence;
    const request = this.request();
    this.held.set({ phase: 'running' });
    this.runner.run(request).then((outcome) => {
      if (started !== this.sequence) {
        return;
      }
      if (outcome === null) {
        this.held.set({ phase: 'refused' });
        this.closed.emit();
      } else if (outcome.ok) {
        this.held.set({ phase: 'done', data: outcome.data });
        this.done.emit(outcome);
      } else {
        this.held.set({ phase: 'failed', error: outcome.error });
      }
    });
  }
}
