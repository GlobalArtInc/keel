import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  computed,
  inject,
  signal,
} from '@angular/core';
import { KeelBoot, KeelCallError, KeelDescriptors, KeelI18n } from '@globalart/keel/client';
import {
  KeelFindingsComponent,
  KeelPageComponent,
  SAVE_KEY,
  buildForm,
  injectKeelWidgets,
  unreachable,
  type FormPage,
  type KeelForm,
} from '@globalart/keel/forms';
import type { CallError, Descriptor } from '@globalart/keel/meta';
import {
  DONE_KEY,
  EMPTY_VALUE,
  FALSE_KEY,
  KeelNavigation,
  NOT_FOUND_KEY,
  RETRY_KEY,
  TRUE_KEY,
  describeError,
  injectKeelViewContext,
} from '@globalart/keel/renderer';
import {
  PhxButtonComponent,
  PhxErrorComponent,
  PhxFormFooterComponent,
  PhxFormRowComponent,
  PhxFormSectionComponent,
  PhxFormSheetComponent,
  PhxLoaderComponent,
  PhxNotFoundComponent,
  PhxPanelComponent,
  PhxToastService,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import {
  SETTINGS_DEFAULT_KEY,
  SETTINGS_GET,
  SETTINGS_SET,
  SETTINGS_TITLE_KEY,
  isSettingRow,
  settingKeyOf,
  valueDescriptor,
  valuesOf,
  type SettingRow,
} from './setting';

export type SettingState =
  | { phase: 'loading' }
  | { phase: 'ready'; row: SettingRow; form: KeelForm }
  | { phase: 'failed'; error: CallError };

const NO_KEY: CallError = { code: 'not_found', key: NOT_FOUND_KEY };

@Component({
  selector: 'keel-settings-value',
  imports: [
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxNotFoundComponent,
    PhxFormSheetComponent,
    PhxFormSectionComponent,
    PhxFormRowComponent,
    PhxFormFooterComponent,
    PhxButtonComponent,
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
        @if (failure().code === 'not_found') {
          <phx-not-found [title]="notFoundTitle()" [text]="message()" [actionLabel]="null" />
        } @else {
          <phx-panel>
            <phx-error [text]="described()" [retryLabel]="retryLabel()" (retry)="load()" />
          </phx-panel>
        }
      }
      @case ('ready') {
        <phx-form-sheet
          [title]="title()"
          [loading]="form().pending()"
          (closed)="close()"
          (submitted)="save()"
        >
          @if (form().unmapped().length) {
            <keel-findings class="banner" [findings]="form().unmapped()" />
          }
          @for (page of form().visiblePages(); track page.id; let last = $last) {
            <phx-form-section [title]="labelOf(page)">
              <keel-page [page]="page" />
              @if (last) {
                <phx-form-row [label]="defaultLabel()">
                  <span class="default" data-default>{{ defaultText() }}</span>
                </phx-form-row>
              }
            </phx-form-section>
          }
          <phx-form-footer>
            <button
              phxButton="primary"
              type="button"
              class="save"
              [loading]="form().pending()"
              [disabled]="form().pending()"
              (click)="save()"
            >
              {{ saveLabel() }}
            </button>
          </phx-form-footer>
        </phx-form-sheet>
      }
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    phx-form-sheet {
      flex: 1 1 auto;
      min-height: 0;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    .banner {
      margin-bottom: var(--phx-form-row-gap, 28px);
    }
    .default {
      min-height: var(--phx-control-height);
      padding-top: calc((var(--phx-control-height) - 18px) / 2);
      font: var(--phx-mono-14);
      color: var(--phx-ink-muted);
      overflow-wrap: anywhere;
    }
  `,
  host: {
    '[attr.data-setting]': 'key',
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelSettingsValueComponent {
  readonly context = injectKeelViewContext();
  readonly key = settingKeyOf(this.context.params);

  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly navigation = inject(KeelNavigation);
  private readonly toasts = inject(PhxToastService);
  private readonly widgets = injectKeelWidgets();
  private readonly held = signal<SettingState>({ phase: 'loading' });
  private sequence = 0;

  readonly state = this.held.asReadonly();

  protected readonly form = computed(() => {
    const state = this.held();
    if (state.phase !== 'ready') {
      throw new TypeError(`the form of the setting ${this.key} is not ready`);
    }
    return state.form;
  });
  protected readonly failure = computed(() => {
    const state = this.held();
    if (state.phase !== 'failed') {
      throw new TypeError(`the setting ${this.key} did not fail`);
    }
    return state.error;
  });
  protected readonly title = computed(() => this.i18n.t(SETTINGS_TITLE_KEY, { key: this.key }));
  protected readonly defaultLabel = computed(() => this.i18n.t(SETTINGS_DEFAULT_KEY));
  protected readonly defaultText = computed(() => {
    const state = this.held();
    return state.phase === 'ready' ? this.render(state.row.default) : EMPTY_VALUE;
  });
  protected readonly saveLabel = computed(() => this.i18n.t(SAVE_KEY));
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly notFoundTitle = computed(() => this.i18n.t(NOT_FOUND_KEY));
  protected readonly message = computed(() => {
    const error = this.failure();
    return this.i18n.t(error.key, error.params);
  });
  protected readonly described = computed(() => describeError(this.i18n, this.failure()));

  constructor() {
    this.load();
    inject(DestroyRef).onDestroy(() => this.drop());
  }

  load(): void {
    const started = ++this.sequence;
    this.drop();
    if (this.key === null) {
      this.held.set({ phase: 'failed', error: NO_KEY });
      return;
    }
    this.held.set({ phase: 'loading' });
    Promise.all([this.descriptor(), this.read(this.key)]).then(
      ([descriptor, row]) => {
        if (started !== this.sequence) {
          return;
        }
        const form = buildForm(valueDescriptor(descriptor, row), {
          mode: 'edit',
          params: this.context.params,
          values: valuesOf(row),
          principal: this.boot.principal(),
          widgets: this.widgets,
          client: this.context.client,
        });
        this.held.set({ phase: 'ready', row, form });
      },
      (error: unknown) => {
        if (started === this.sequence) {
          this.held.set({
            phase: 'failed',
            error: error instanceof KeelCallError ? error.error : unreachable(error),
          });
        }
      },
    );
  }

  async save(): Promise<void> {
    const state = this.held();
    if (state.phase !== 'ready' || state.form.pending()) {
      return;
    }
    const envelope = await state.form.submit();
    if (envelope === null || !envelope.ok) {
      return;
    }
    this.toasts.success(this.i18n.t(DONE_KEY, { action: this.title() }));
    this.navigation.back();
  }

  close(): void {
    this.navigation.back();
  }

  protected labelOf(page: FormPage): string {
    return this.i18n.t(page.labelKey);
  }

  private descriptor(): Promise<Descriptor> {
    const given = this.context.descriptor;
    return given === undefined ? this.descriptors.get(SETTINGS_SET) : Promise.resolve(given);
  }

  private async read(key: string): Promise<SettingRow> {
    const envelope = await firstValueFrom(this.context.client.call(SETTINGS_GET, { key }));
    if (!envelope.ok) {
      throw new KeelCallError(envelope.error);
    }
    if (!isSettingRow(envelope.data)) {
      throw new TypeError(`${SETTINGS_GET} answered no row for ${key}`);
    }
    return envelope.data;
  }

  private render(value: unknown): string {
    switch (typeof value) {
      case 'undefined':
        return EMPTY_VALUE;
      case 'string':
        return value;
      case 'boolean':
        return this.i18n.t(value ? TRUE_KEY : FALSE_KEY);
      case 'number':
      case 'bigint':
        return String(value);
      default:
        return value === null ? EMPTY_VALUE : JSON.stringify(value);
    }
  }

  private drop(): void {
    const state = this.held();
    if (state.phase === 'ready') {
      state.form.destroy();
    }
  }
}
