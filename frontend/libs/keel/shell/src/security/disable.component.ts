import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { ReactiveFormsModule } from '@angular/forms';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Envelope } from '@globalart/keel/meta';
import {
  PhxButtonComponent,
  PhxFormButtonsComponent,
  PhxInputComponent,
  PhxModalRef,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { codeErrorOf, codeForm } from './code';
import { CODE_LABEL_KEY, SECURITY_DISABLE_KEY, TOTP_DISABLE } from './totp';

@Component({
  selector: 'keel-totp-disable',
  imports: [ReactiveFormsModule, PhxButtonComponent, PhxFormButtonsComponent, PhxInputComponent],
  template: `
    <form class="form" novalidate [formGroup]="form" (ngSubmit)="disable()">
      <phx-input
        formControlName="code"
        width="full"
        autocomplete="one-time-code"
        inputmode="numeric"
        [label]="codeLabel()"
        [error]="error()"
      />
      <phx-form-buttons align="end">
        <button
          phxButton="danger"
          type="submit"
          class="disable"
          [loading]="busy()"
          [disabled]="busy()"
        >
          {{ disableLabel() }}
        </button>
      </phx-form-buttons>
    </form>
  `,
  styles: `
    :host {
      display: block;
    }
    .form {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelTotpDisableComponent {
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly ref = inject(PhxModalRef);
  private readonly working = signal(false);
  private readonly refusal = signal<string | null>(null);

  readonly form = codeForm();
  readonly busy = this.working.asReadonly();
  readonly error = this.refusal.asReadonly();

  protected readonly codeLabel = computed(() => this.i18n.t(CODE_LABEL_KEY));
  protected readonly disableLabel = computed(() => this.i18n.t(SECURITY_DISABLE_KEY));

  async disable(): Promise<void> {
    this.form.markAllAsTouched();
    if (this.busy() || this.form.invalid) {
      return;
    }
    this.working.set(true);
    this.refusal.set(null);
    try {
      const envelope = await firstValueFrom(
        this.client.call(TOTP_DISABLE, this.form.getRawValue()),
      ).catch((error: unknown): Envelope => ({ ok: false, error: unreachable(error) }));
      if (envelope.ok) {
        this.ref.close(true);
      } else {
        this.refusal.set(codeErrorOf(this.i18n, envelope.error));
      }
    } finally {
      this.working.set(false);
    }
  }
}
