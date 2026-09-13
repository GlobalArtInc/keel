import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
} from '@angular/core';
import { KeelBoot, KeelCallError, KeelClient, KeelI18n } from '@globalart/keel/client';
import {
  KeelFormComponent,
  injectKeelWidgets,
  loadForm,
  unreachable,
  type KeelForm,
  type Saved,
} from '@globalart/keel/forms';
import type { CallError, Descriptor } from '@globalart/keel/meta';
import { PhxErrorComponent, PhxLoaderComponent, PhxPanelComponent } from '@globalart/phoenix-ui';
import { RETRY_KEY, describeError } from '../messages';

export type FormScreenState =
  { phase: 'loading' } | { phase: 'ready'; form: KeelForm } | { phase: 'failed'; error: CallError };

@Component({
  selector: 'keel-form-screen',
  imports: [KeelFormComponent, PhxPanelComponent, PhxLoaderComponent, PhxErrorComponent],
  template: `
    @switch (state().phase) {
      @case ('loading') {
        <phx-panel
          ><div class="loading"><phx-loader [size]="24" /></div
        ></phx-panel>
      }
      @case ('failed') {
        <phx-panel>
          <phx-error [text]="message()" [retryLabel]="retryLabel()" (retry)="reload()" />
        </phx-panel>
      }
      @case ('ready') {
        <keel-form [form]="form()" (closed)="closed.emit()" (submitted)="saved.emit($event)" />
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
    '[attr.data-form-screen]': 'descriptor().name',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelFormScreenComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});

  readonly saved = output<Saved>();
  readonly closed = output<void>();

  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly widgets = injectKeelWidgets();
  private readonly held = signal<FormScreenState>({ phase: 'loading' });
  private sequence = 0;

  readonly state = this.held.asReadonly();

  protected readonly form = computed(() => {
    const state = this.state();
    if (state.phase !== 'ready') {
      throw new TypeError(`the form of ${this.descriptor().name} is not ready`);
    }
    return state.form;
  });
  protected readonly message = computed(() => {
    const state = this.state();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));

  constructor() {
    effect(() => {
      this.descriptor();
      this.params();
      untracked(() => this.reload());
    });
    inject(DestroyRef).onDestroy(() => this.drop());
  }

  reload(): void {
    const started = ++this.sequence;
    const descriptor = this.descriptor();
    const params = this.params();
    this.drop();
    this.held.set({ phase: 'loading' });
    loadForm(descriptor, {
      client: this.client,
      params,
      principal: this.boot.principal(),
      widgets: this.widgets,
    }).then(
      (form) => {
        if (started !== this.sequence) {
          form.destroy();
          return;
        }
        this.held.set({ phase: 'ready', form });
      },
      (error: unknown) => {
        if (started !== this.sequence) {
          return;
        }
        this.held.set({
          phase: 'failed',
          error: error instanceof KeelCallError ? error.error : unreachable(error),
        });
      },
    );
  }

  private drop(): void {
    const state = this.held();
    if (state.phase === 'ready') {
      state.form.destroy();
    }
  }
}
