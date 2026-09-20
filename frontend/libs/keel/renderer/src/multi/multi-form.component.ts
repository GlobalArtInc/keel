import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  Injector,
  computed,
  inject,
  signal,
} from '@angular/core';
import { KeelBoot, KeelClient, KeelI18n, isTaskRef } from '@globalart/keel/client';
import {
  KeelFieldComponent,
  KeelFindingsComponent,
  applyFieldErrors,
  buildForm,
  clearFieldErrors,
  isEmptyValue,
  newIdempotencyKey,
  unreachable,
  type FormField,
  type KeelForm,
} from '@globalart/keel/forms';
import type { Descriptor, Envelope, FieldError, GroupResult } from '@globalart/keel/meta';
import {
  PHX_MODAL_DATA,
  PhxButtonComponent,
  PhxModalRef,
  PhxModalService,
  PhxToastService,
  PhxToggleComponent,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { KeelTasks } from '../tasks/follow';

export const MULTI_TITLE_KEY = 'keel.shell.multi.title';
const MULTI_CHANGE_KEY = 'keel.shell.multi.change';
const MULTI_SAVE_KEY = 'keel.shell.multi.save';
const MULTI_CANCEL_KEY = 'keel.shell.multi.cancel';
const MULTI_NONE_KEY = 'keel.shell.multi.none';
const MULTI_UPDATED_KEY = 'keel.shell.multi.updated';
const MULTI_MODAL_WIDTH = 560;

const MASK_PATH = /^mask\[(\d+)\]$/;
const FIELDS_PREFIX = 'fields.';

export interface MultiFormRequest {
  descriptor: Descriptor;
  ids: readonly string[];
  title: string;
}

export interface GroupBody {
  ids: string[];
  fields: Record<string, unknown>;
  mask: string[];
}

export type MultiOutcome = { kind: 'updated'; updated: number } | { kind: 'task' };

export function multiNamesOf(descriptor: Descriptor): readonly string[] {
  return descriptor.ui?.multi ?? [];
}

export function isMultiForm(descriptor: Descriptor): boolean {
  return descriptor.kind === 'form' && multiNamesOf(descriptor).length > 0;
}

export function multiDescriptorOf(descriptor: Descriptor): Descriptor {
  const ui = descriptor.ui;
  return ui ? { ...descriptor, ui: { ...ui, pages: [], fields: {} } } : descriptor;
}

export function groupBodyOf(
  ids: readonly string[],
  fields: readonly FormField[],
  mask: ReadonlySet<string>,
): GroupBody {
  const body: GroupBody = { ids: [...ids], fields: {}, mask: [] };
  for (const field of fields) {
    if (!mask.has(field.name)) {
      continue;
    }
    const value = field.control.value;
    const blank = isEmptyValue(value) && field.nullable;
    body.fields[field.name] = blank ? null : (value ?? null);
    body.mask.push(field.name);
  }
  return body;
}

export function groupPathOf(path: string, mask: readonly string[]): string {
  if (path.startsWith(FIELDS_PREFIX)) {
    return path.slice(FIELDS_PREFIX.length);
  }
  const indexed = MASK_PATH.exec(path);
  if (indexed !== null) {
    return mask[Number(indexed[1])] ?? path;
  }
  return path;
}

export function isGroupResult(data: unknown): data is GroupResult {
  return (
    typeof data === 'object' &&
    data !== null &&
    typeof (data as { updated?: unknown }).updated === 'number'
  );
}

@Component({
  selector: 'keel-multi-form',
  imports: [PhxToggleComponent, PhxButtonComponent, KeelFieldComponent, KeelFindingsComponent],
  template: `
    @if (form.unmapped().length) {
      <keel-findings class="banner" [findings]="form.unmapped()" />
    }
    @for (field of fields; track field.name) {
      <div class="row" [attr.data-multi]="field.name">
        <phx-toggle
          class="mask"
          [checked]="masked().has(field.name)"
          [ariaLabel]="changeLabel() + ': ' + labelOf(field)"
          (checkedChange)="toggle(field, $event)"
        />
        <keel-field class="field" [field]="field" />
      </div>
    }
    @if (none()) {
      <p class="none">{{ noneLabel() }}</p>
    }
    <div class="footer">
      <button phxButton type="button" class="cancel" [disabled]="pending()" (click)="cancel()">
        {{ cancelLabel() }}
      </button>
      <button
        phxButton="primary"
        type="button"
        class="save"
        [loading]="pending()"
        [disabled]="pending() || masked().size === 0"
        (click)="submit()"
      >
        {{ saveLabel() }}
      </button>
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
    .row {
      display: flex;
      gap: 12px;
      align-items: flex-start;
    }
    .mask {
      flex: 0 0 auto;
      margin-top: 4px;
    }
    .field {
      flex: 1 1 auto;
      min-width: 0;
    }
    .none {
      margin: 0;
      font: var(--phx-text-12);
      color: var(--phx-ink-muted);
    }
    .footer {
      display: flex;
      gap: 8px;
      justify-content: flex-end;
    }
  `,
  host: {
    '[attr.data-multi-form]': 'request.descriptor.name',
    '[attr.data-count]': 'request.ids.length',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelMultiFormComponent {
  protected readonly request = inject<MultiFormRequest>(PHX_MODAL_DATA);

  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly ref = inject<PhxModalRef<MultiOutcome | undefined>>(PhxModalRef);
  private readonly tasks = inject(KeelTasks);
  private readonly toasts = inject(PhxToastService);
  private idempotencyKey: string | null = null;

  readonly form: KeelForm = buildForm(multiDescriptorOf(this.request.descriptor), {
    mode: 'edit',
    principal: this.boot.principal(),
    client: this.client,
  });
  readonly fields: readonly FormField[] = multiNamesOf(this.request.descriptor).flatMap((name) => {
    const field = this.form.field(name);
    return field === undefined ? [] : [field];
  });
  readonly masked = signal<ReadonlySet<string>>(new Set());
  readonly pending = signal(false);

  protected readonly none = computed(() => this.masked().size === 0);
  protected readonly changeLabel = computed(() => this.i18n.t(MULTI_CHANGE_KEY));
  protected readonly saveLabel = computed(() => this.i18n.t(MULTI_SAVE_KEY));
  protected readonly cancelLabel = computed(() => this.i18n.t(MULTI_CANCEL_KEY));
  protected readonly noneLabel = computed(() => this.i18n.t(MULTI_NONE_KEY));

  constructor() {
    for (const field of this.fields) {
      field.control.disable({ emitEvent: false });
    }
    inject(DestroyRef).onDestroy(() => this.form.destroy());
  }

  protected labelOf(field: FormField): string {
    return this.i18n.t(field.labelKey);
  }

  toggle(field: FormField, on: boolean): void {
    const next = new Set(this.masked());
    if (on) {
      next.add(field.name);
      field.control.enable();
    } else {
      next.delete(field.name);
      field.control.disable();
    }
    this.masked.set(next);
  }

  cancel(): void {
    this.ref.close(undefined);
  }

  async submit(): Promise<void> {
    const mask = this.masked();
    if (mask.size === 0 || this.pending()) {
      return;
    }
    clearFieldErrors(this.form);
    for (const field of this.fields) {
      if (mask.has(field.name)) {
        field.control.markAsTouched();
      }
    }
    if (this.fields.some((field) => mask.has(field.name) && field.control.invalid)) {
      return;
    }
    const body = groupBodyOf(this.request.ids, this.fields, mask);
    this.idempotencyKey ??= newIdempotencyKey();
    this.pending.set(true);
    const envelope = await firstValueFrom(
      this.client.call(this.request.descriptor.name, body, {
        idempotencyKey: this.idempotencyKey,
      }),
    ).catch((error: unknown): Envelope => ({ ok: false, error: unreachable(error) }));
    this.pending.set(false);
    if (!envelope.ok) {
      const fields = (envelope.error.fields ?? []).map((entry): FieldError => ({
        ...entry,
        path: groupPathOf(entry.path, body.mask),
      }));
      applyFieldErrors(this.form, fields.length > 0 ? fields : [{ path: '' }], envelope.error);
      return;
    }
    this.idempotencyKey = null;
    if (isTaskRef(envelope.data)) {
      this.ref.close({ kind: 'task' });
      await this.tasks.follow(envelope.data, { title: this.request.title });
      return;
    }
    const updated = isGroupResult(envelope.data) ? envelope.data.updated : this.request.ids.length;
    this.toasts.success(this.i18n.t(MULTI_UPDATED_KEY, { count: updated }), {
      title: this.request.title,
    });
    this.ref.close({ kind: 'updated', updated });
  }
}

export function openMultiForm(
  modals: PhxModalService,
  injector: Injector,
  request: MultiFormRequest,
): Promise<MultiOutcome | undefined> {
  const ref = modals.open<KeelMultiFormComponent, MultiFormRequest, MultiOutcome>(
    KeelMultiFormComponent,
    { title: request.title, width: MULTI_MODAL_WIDTH, data: request, injector },
  );
  return firstValueFrom(ref.afterClosed);
}
