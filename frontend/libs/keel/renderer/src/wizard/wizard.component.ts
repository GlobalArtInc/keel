import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  booleanAttribute,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  untracked,
} from '@angular/core';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import {
  KeelFindingsComponent,
  KeelPageComponent,
  injectKeelWidgets,
  type FormPage,
  type Saved,
} from '@globalart/keel/forms';
import type { Descriptor } from '@globalart/keel/meta';
import {
  PhxButtonComponent,
  PhxErrorComponent,
  PhxFormFooterComponent,
  PhxFormSheetComponent,
  PhxLoaderComponent,
  PhxPanelComponent,
  PhxWizardComponent,
  type PhxWizardStep,
} from '@globalart/phoenix-ui';
import { formLoader } from '../form/loader';
import { BACK_KEY, FINISH_KEY, NEXT_KEY, RETRY_KEY, describeError } from '../messages';
import { KeelNavigation } from '../navigation';
import { firstInvalidPage, stepsOf, validateStep } from './steps';

const NO_PAGES: readonly FormPage[] = [];

@Component({
  selector: 'keel-wizard',
  imports: [
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxFormSheetComponent,
    PhxFormFooterComponent,
    PhxButtonComponent,
    PhxWizardComponent,
    KeelFindingsComponent,
    KeelPageComponent,
  ],
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
        <phx-form-sheet
          [title]="title()"
          [closable]="closable()"
          [loading]="form().pending()"
          (closed)="closed.emit()"
          (submitted)="next()"
        >
          <phx-wizard
            class="steps"
            [steps]="steps()"
            [active]="activeId()"
            [label]="title()"
            [disabled]="busy()"
            numbered
            full
            (change)="onStep($event)"
          />
          @if (form().unmapped().length) {
            <keel-findings class="banner" [findings]="form().unmapped()" />
          }
          @if (page(); as page) {
            <keel-page [page]="page" />
          }
          <phx-form-footer>
            <button
              phxButton
              type="button"
              class="back"
              icon="arrow-left"
              [disabled]="first() || busy()"
              (click)="back()"
            >
              {{ backLabel() }}
            </button>
            <button
              phxButton="primary"
              type="button"
              class="next"
              [loading]="busy()"
              [disabled]="busy()"
              (click)="next()"
            >
              {{ last() ? finishLabel() : nextLabel() }}
            </button>
          </phx-form-footer>
        </phx-form-sheet>
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
    phx-form-sheet {
      flex: 1 1 auto;
      min-height: 0;
    }
    .steps,
    .banner {
      margin-bottom: var(--phx-form-row-gap, 28px);
    }
  `,
  host: {
    '[attr.data-wizard]': 'descriptor().name',
    '[attr.data-phase]': 'state().phase',
    '[attr.data-step]': 'activeId()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelWizardComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly params = input<Record<string, unknown>>({});
  readonly closable = input(true, { transform: booleanAttribute });

  readonly done = output<Saved>();
  readonly closed = output<void>();

  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly navigation = inject(KeelNavigation);
  private readonly widgets = injectKeelWidgets();
  private readonly loader = formLoader(() => ({
    client: this.client,
    principal: this.boot.principal(),
    widgets: this.widgets,
  }));
  private readonly validating = signal(false);

  readonly state = this.loader.state;
  readonly active = signal(0);

  protected readonly form = computed(() => {
    const state = this.state();
    if (state.phase !== 'ready') {
      throw new TypeError(`the wizard ${this.descriptor().name} is not ready`);
    }
    return state.form;
  });
  readonly pages = computed(() => {
    const state = this.state();
    return state.phase === 'ready' ? state.form.visiblePages() : NO_PAGES;
  });
  readonly page = computed<FormPage | null>(() => this.pages()[this.active()] ?? null);
  readonly first = computed(() => this.active() <= 0);
  readonly last = computed(() => this.active() >= this.pages().length - 1);
  readonly busy = computed(() => {
    const state = this.state();
    return this.validating() || (state.phase === 'ready' && state.form.pending());
  });
  readonly steps = computed<PhxWizardStep[]>(() =>
    stepsOf(this.pages(), this.active(), (page) => this.labelOf(page)),
  );
  protected readonly activeId = computed(() => this.page()?.id ?? null);
  protected readonly title = computed(() => {
    const descriptor = this.descriptor();
    return this.i18n.t(descriptor.ui?.title ?? descriptor.name);
  });
  protected readonly message = computed(() => {
    const state = this.state();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly backLabel = computed(() => this.i18n.t(BACK_KEY));
  protected readonly nextLabel = computed(() => this.i18n.t(NEXT_KEY));
  protected readonly finishLabel = computed(() => this.i18n.t(FINISH_KEY));

  constructor() {
    effect(() => {
      const descriptor = this.descriptor();
      const params = this.params();
      untracked(() => this.loader.reload(descriptor, params));
    });
    effect(() => {
      this.state();
      untracked(() => this.active.set(0));
    });
    effect(() => {
      const last = this.pages().length - 1;
      if (this.active() > last) {
        this.active.set(Math.max(0, last));
      }
    });
    inject(DestroyRef).onDestroy(() => this.loader.drop());
  }

  reload(): void {
    this.loader.reload(this.descriptor(), this.params());
  }

  back(): void {
    if (!this.first() && !this.busy()) {
      this.active.set(this.active() - 1);
    }
  }

  async next(): Promise<void> {
    if (this.busy() || this.state().phase !== 'ready') {
      return;
    }
    const form = this.form();
    const pages = this.pages();
    const index = this.active();
    this.validating.set(true);
    let valid: boolean;
    try {
      valid = await validateStep(form, pages, index, this.client);
    } finally {
      this.validating.set(false);
    }
    if (!valid) {
      return;
    }
    if (index < pages.length - 1) {
      this.active.set(index + 1);
      return;
    }
    const envelope = await form.submit();
    if (envelope === null || !envelope.ok) {
      const failed = firstInvalidPage(pages);
      if (failed >= 0) {
        this.active.set(failed);
      }
      return;
    }
    this.done.emit(envelope);
    if (form.descriptor.ui?.result === 'redirect') {
      this.navigation.back();
    }
  }

  protected onStep(step: PhxWizardStep): void {
    const index = this.pages().findIndex((page) => page.id === step.id);
    if (index >= 0 && index < this.active()) {
      this.active.set(index);
    }
  }

  private labelOf(page: FormPage): string {
    return this.i18n.t(page.labelKey);
  }
}
