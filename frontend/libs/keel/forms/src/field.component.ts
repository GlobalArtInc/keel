import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  forwardRef,
  inject,
  input,
  signal,
  untracked,
} from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  FormControl,
  ReactiveFormsModule,
  TouchedChangeEvent,
  ValueChangeEvent,
} from '@angular/forms';
import { KeelI18n } from '@globalart/keel/client';
import { lookup, type Choice, type FieldError } from '@globalart/keel/meta';
import {
  PHX_LABELS,
  PhxDateFieldComponent,
  PhxFieldErrorComponent,
  PhxFieldHintComponent,
  PhxFileFieldComponent,
  PhxFormRowComponent,
  PhxInputComponent,
  PhxKeyValueFieldComponent,
  PhxListFieldComponent,
  PhxNumberFieldComponent,
  PhxPasswordComponent,
  PhxSelectComponent,
  PhxSliderComponent,
  PhxSwitcherComponent,
  PhxTagsFieldComponent,
  PhxTimeFieldComponent,
  PhxToggleComponent,
  PhxTransferListComponent,
  type PhxColumn,
  type PhxDateValue,
  type PhxLabels,
  type PhxSelectOption,
  type PhxSwitcherOption,
  type PhxTransferItem,
} from '@globalart/phoenix-ui';
import { PhxCodeEditorComponent } from '@globalart/phoenix-ui/code';
import type { ControlMapping } from './mapping';
import type { FormField } from './model';
import { toSelectOption } from './options';
import { findingsOf } from './validators';

interface ControlState {
  value: unknown;
  disabled: boolean;
  shown: boolean;
  findings: FieldError[];
}

const ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

const EMPTY_SUFFIX = '.empty';
const UNLIMITED_KEY = 'phx.unlimited';

@Component({
  selector: 'keel-field',
  imports: [
    ReactiveFormsModule,
    PhxFormRowComponent,
    PhxFieldErrorComponent,
    PhxFieldHintComponent,
    PhxInputComponent,
    PhxPasswordComponent,
    PhxSelectComponent,
    PhxToggleComponent,
    PhxSwitcherComponent,
    PhxSliderComponent,
    PhxNumberFieldComponent,
    PhxDateFieldComponent,
    PhxTimeFieldComponent,
    PhxFileFieldComponent,
    PhxListFieldComponent,
    PhxCodeEditorComponent,
    PhxTagsFieldComponent,
    PhxKeyValueFieldComponent,
    PhxTransferListComponent,
  ],
  template: `
    <phx-form-row [label]="label()" [required]="required()">
      @switch (component()) {
        @case ('phx-input') {
          <phx-input
            [formControl]="control()"
            [type]="inputType()"
            [placeholder]="placeholder()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-password') {
          <phx-password
            [formControl]="control()"
            [showGenerate]="generate()"
            [placeholder]="placeholder()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-select') {
          <phx-select
            [formControl]="control()"
            [options]="choices()"
            [remote]="remote() !== null"
            [loading]="loading()"
            [more]="more()"
            (searchChange)="search($event)"
            (loadMore)="loadMore()"
            [placeholder]="placeholder()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-toggle') {
          <phx-toggle [formControl]="control()" [required]="required()" [invalid]="invalid()" />
        }
        @case ('phx-switcher') {
          <phx-switcher
            [formControl]="control()"
            [options]="switcherOptions()"
            [required]="required()"
            [invalid]="invalid()"
          />
        }
        @case ('phx-slider') {
          <phx-slider
            [formControl]="control()"
            [min]="min() ?? 0"
            [max]="max() ?? 100"
            [step]="step()"
            [unlimited]="unlimited()"
            [unlimitedValue]="null"
            [required]="required()"
            [invalid]="invalid()"
            width="full"
          />
          <phx-number-field
            [formControl]="mirror"
            [min]="min()"
            [max]="max()"
            [step]="step()"
            [unlimited]="unlimited()"
            [required]="required()"
            [invalid]="invalid()"
          />
        }
        @case ('phx-number-field') {
          <phx-number-field
            [formControl]="control()"
            [min]="min()"
            [max]="max()"
            [step]="step()"
            [unlimited]="unlimited()"
            [placeholder]="placeholder()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
          />
        }
        @case ('phx-date-field') {
          <phx-date-field
            [value]="date()"
            (change)="setDate($event)"
            [placeholder]="placeholder()"
            [disabled]="disabled()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
          />
        }
        @case ('phx-time-field') {
          <phx-time-field
            [formControl]="control()"
            [placeholder]="placeholder()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
          />
        }
        @case ('phx-file-field') {
          <phx-file-field
            [formControl]="control()"
            [label]="label()"
            [accept]="accept()"
            [maxSize]="maxSize()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-list-field') {
          <phx-list-field
            [formControl]="control()"
            [columns]="columns()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-code-editor') {
          @defer (on immediate) {
            <phx-code-editor
              [formControl]="control()"
              [placeholder]="placeholder()"
              [required]="required()"
              [hint]="hint()"
              [error]="error()"
            />
          }
        }
        @case ('phx-tags-field') {
          <phx-tags-field
            [formControl]="control()"
            [placeholder]="placeholder()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-key-value-field') {
          <phx-key-value-field
            [formControl]="control()"
            [keyPattern]="keyPattern()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
        @case ('phx-transfer-list') {
          <phx-transfer-list
            [formControl]="control()"
            [items]="transferItems()"
            [loading]="loading()"
            [required]="required()"
            [hint]="hint()"
            [error]="error()"
            width="full"
          />
        }
      }
      @if (bare()) {
        @if (hint(); as hint) {
          <phx-field-hint [text]="hint" />
        }
        @if (error(); as error) {
          <phx-field-error [text]="error" />
        }
      }
    </phx-form-row>
  `,
  styles: `
    :host {
      display: block;
    }
  `,
  providers: [
    {
      provide: PHX_LABELS,
      useFactory: (host: KeelFieldComponent): PhxLabels => host.labels,
      deps: [forwardRef(() => KeelFieldComponent)],
    },
  ],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelFieldComponent {
  readonly field = input.required<FormField>();

  protected readonly mirror = new FormControl<number | null>(null);

  private readonly i18n = inject(KeelI18n);
  private readonly parentLabels = inject(PHX_LABELS, { skipSelf: true });
  private readonly version = signal(0);

  readonly emptyLabel = computed(() => {
    const messages = this.i18n.messages();
    return (
      lookup(messages, `${this.field().labelKey}${EMPTY_SUFFIX}`) ??
      lookup(messages, UNLIMITED_KEY) ??
      this.parentLabels.unlimited
    );
  });
  readonly labels: PhxLabels = scopedLabels(this.parentLabels, this.emptyLabel);

  protected readonly control = computed(() => this.field().control);
  protected readonly mapping = computed(() => this.field().mapping);
  protected readonly component = computed(() => this.mapping().component);
  protected readonly required = computed(() => this.field().required());
  protected readonly label = computed(() => this.i18n.t(this.field().labelKey));
  protected readonly hint = computed(() => {
    const check = this.field().checkHintKey();
    if (check !== null && this.i18n.has(check)) {
      return this.i18n.t(check);
    }
    return this.translate(this.field().hintKey);
  });
  protected readonly placeholder = computed(() => this.translate(this.field().placeholderKey));
  protected readonly state = computed<ControlState>(() => {
    this.version();
    const control = this.field().control;
    return {
      value: control.value,
      disabled: control.disabled,
      shown: control.touched || control.dirty,
      findings: findingsOf(control),
    };
  });
  protected readonly disabled = computed(() => this.state().disabled);
  protected readonly error = computed(() => {
    const { shown, findings } = this.state();
    const finding = findings[0];
    if (!shown || finding === undefined) {
      return null;
    }
    return this.i18n.t(finding.key ?? finding.code ?? 'keel.error.validation', finding.params);
  });
  protected readonly invalid = computed(() => this.error() !== null);
  protected readonly bare = computed(() => {
    const component = this.component();
    return component === 'phx-toggle' || component === 'phx-switcher' || component === 'phx-slider';
  });
  protected readonly inputType = computed(
    () => member(this.mapping(), 'phx-input')?.type ?? 'text',
  );
  protected readonly generate = computed(
    () => member(this.mapping(), 'phx-password')?.generate ?? false,
  );
  protected readonly remote = computed(() => member(this.mapping(), 'phx-select')?.remote ?? null);
  protected readonly choices = computed<PhxSelectOption<string>[]>(() => {
    const served = this.field().choices();
    if (served !== null) {
      return served.map(toSelectOption);
    }
    const options = this.field().options;
    if (options !== null) {
      return options.options();
    }
    return (member(this.mapping(), 'phx-select')?.choices ?? []).map((value) => this.choice(value));
  });
  protected readonly loading = computed(() => this.field().options?.loading() ?? false);
  protected readonly more = computed(() => this.field().options?.more() ?? false);
  protected readonly switcherOptions = computed<PhxSwitcherOption<string>[]>(() => {
    const served = this.field().choices();
    if (served !== null) {
      return served.map(toSelectOption);
    }
    return (member(this.mapping(), 'phx-switcher')?.choices ?? []).map((value) =>
      this.choice(value),
    );
  });
  protected readonly accept = computed(() => {
    const types = member(this.mapping(), 'phx-file-field')?.accept ?? [];
    return types.length === 0 ? null : types.join(',');
  });
  protected readonly maxSize = computed(
    () => member(this.mapping(), 'phx-file-field')?.maxSize ?? null,
  );
  protected readonly min = computed(() => this.numeric()?.min ?? null);
  protected readonly max = computed(() => this.numeric()?.max ?? null);
  protected readonly step = computed(() => this.numeric()?.step ?? 1);
  protected readonly unlimited = computed(() => this.numeric()?.unlimited ?? false);
  protected readonly number = computed(() => {
    const value = this.state().value;
    return typeof value === 'number' ? value : null;
  });
  protected readonly date = computed<PhxDateValue>(() => {
    const value = this.state().value;
    return typeof value === 'string' ? parseIsoDate(value) : null;
  });
  protected readonly columns = computed<PhxColumn[]>(() =>
    (member(this.mapping(), 'phx-list-field')?.columns ?? []).map((key) => ({
      key,
      label: this.i18n.t(`${this.field().labelKey}.${key}`),
      editable: true,
    })),
  );

  protected readonly keyPattern = computed(
    () => member(this.mapping(), 'phx-key-value-field')?.keyPattern ?? null,
  );
  protected readonly transferItems = computed<PhxTransferItem[]>(() => {
    const served = this.field().choices() ?? this.field().options?.choices() ?? null;
    if (served !== null) {
      return served.map(toTransferItem);
    }
    return (member(this.mapping(), 'phx-transfer-list')?.choices ?? []).map((value) => ({
      id: value,
      label: this.choice(value).label,
    }));
  });

  private readonly numeric = computed(
    () => member(this.mapping(), 'phx-slider') ?? member(this.mapping(), 'phx-number-field'),
  );

  constructor() {
    effect((onCleanup) => {
      const subscription = this.field().control.events.subscribe(() =>
        this.version.update((version) => version + 1),
      );
      onCleanup(() => subscription.unsubscribe());
    });
    effect(() => {
      const value = this.number();
      const disabled = this.disabled();
      untracked(() => this.reflect(value, disabled));
    });
    effect(() => {
      const options = member(this.mapping(), 'phx-transfer-list') ? this.field().options : null;
      untracked(() => options?.search(''));
    });
    this.mirror.events.pipe(takeUntilDestroyed()).subscribe((event) => {
      if (event instanceof ValueChangeEvent && event.value !== this.control().value) {
        this.setValue(event.value);
      } else if (event instanceof TouchedChangeEvent && event.touched) {
        this.control().markAsTouched();
      }
    });
  }

  protected setValue(value: unknown): void {
    const control = this.control();
    control.markAsDirty();
    control.markAsTouched();
    control.setValue(value);
  }

  protected setDate(value: PhxDateValue): void {
    this.setValue(value instanceof Date ? formatIsoDate(value) : null);
  }

  protected search(query: string): void {
    this.field().options?.search(query);
  }

  protected loadMore(): void {
    this.field().options?.loadMore();
  }

  private reflect(value: number | null, disabled: boolean): void {
    if (this.mirror.value !== value) {
      this.mirror.setValue(value, { emitEvent: false });
    }
    if (this.mirror.disabled !== disabled) {
      if (disabled) {
        this.mirror.disable({ emitEvent: false });
      } else {
        this.mirror.enable({ emitEvent: false });
      }
    }
  }

  private translate(key: string | null): string | null {
    return key === null ? null : this.i18n.t(key);
  }

  private choice(value: string): { value: string; label: string } {
    return { value, label: this.i18n.t(`${this.field().labelKey}.${value}`) };
  }
}

function toTransferItem(choice: Choice): PhxTransferItem {
  return { id: choice.value, label: choice.label, hint: choice.hint ?? null };
}

function member<C extends ControlMapping['component']>(
  mapping: ControlMapping,
  component: C,
): Extract<ControlMapping, { component: C }> | null {
  return mapping.component === component
    ? (mapping as Extract<ControlMapping, { component: C }>)
    : null;
}

function parseIsoDate(value: string): Date | null {
  const match = ISO_DATE.exec(value);
  if (match === null) {
    return null;
  }
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  return Number.isNaN(date.getTime()) ? null : date;
}

function formatIsoDate(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${date.getFullYear()}-${month}-${day}`;
}

function scopedLabels(parent: PhxLabels, unlimited: () => string): PhxLabels {
  return Object.defineProperty({ ...parent }, 'unlimited', {
    enumerable: true,
    get: unlimited,
  });
}
