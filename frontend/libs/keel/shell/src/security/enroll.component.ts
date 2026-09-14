import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { ReactiveFormsModule } from '@angular/forms';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Envelope } from '@globalart/keel/meta';
import { FINISH_KEY } from '@globalart/keel/renderer';
import {
  PhxBannerComponent,
  PhxButtonComponent,
  PhxClipboardComponent,
  PhxFormButtonsComponent,
  PhxInputComponent,
  PhxModalRef,
  PhxPropComponent,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { codeErrorOf, codeForm } from './code';
import {
  CODE_LABEL_KEY,
  SECURITY_CONFIRM_KEY,
  SECURITY_RECOVERY_HINT_KEY,
  SECURITY_RECOVERY_KEY,
  SECURITY_SECRET_HINT_KEY,
  SECURITY_SECRET_KEY,
  TOTP_CONFIRM,
  isEnrolled,
  qrOf,
  recoveryCodesOf,
  viewBoxOf,
  type Enrolled,
} from './totp';

export type EnrollStage = { kind: 'confirm' } | { kind: 'recovery'; codes: string[] };

const CONFIRM_STAGE: EnrollStage = { kind: 'confirm' };

@Component({
  selector: 'keel-totp-enroll',
  imports: [
    ReactiveFormsModule,
    PhxBannerComponent,
    PhxButtonComponent,
    PhxClipboardComponent,
    PhxFormButtonsComponent,
    PhxInputComponent,
    PhxPropComponent,
  ],
  template: `
    @if (stage().kind === 'recovery') {
      <phx-banner status="warning" role="status" [title]="recoveryLabel()">
        {{ recoveryHint() }}
      </phx-banner>
      <ol class="codes" data-recovery [attr.aria-label]="recoveryLabel()">
        @for (code of codes(); track code) {
          <li class="code">{{ code }}</li>
        }
      </ol>
      <phx-form-buttons align="between">
        <phx-clipboard
          size="default"
          appearance="outlined"
          data-copy="recovery"
          [text]="codesText()"
        />
        <button phxButton="primary" type="button" class="finish" (click)="finish()">
          {{ finishLabel() }}
        </button>
      </phx-form-buttons>
    } @else {
      <form class="form" novalidate [formGroup]="form" (ngSubmit)="confirm()">
        <p class="hint">{{ secretHint() }}</p>
        <figure class="qr">
          <svg
            class="qr__image"
            data-qr
            role="img"
            [attr.viewBox]="viewBox()"
            [attr.aria-label]="enrolled.uri"
          >
            <path class="qr__modules" [attr.d]="qr().path" />
          </svg>
          <phx-clipboard class="qr__copy" data-copy="uri" [text]="enrolled.uri" />
        </figure>
        <phx-prop
          class="secret"
          data-secret
          mono
          copyable
          borderless
          stacked
          [label]="secretLabel()"
          [value]="enrolled.secret"
        />
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
            phxButton="primary"
            type="submit"
            class="confirm"
            [loading]="busy()"
            [disabled]="busy()"
          >
            {{ confirmLabel() }}
          </button>
        </phx-form-buttons>
      </form>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
    .form {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
    .hint {
      margin: 0;
      color: var(--phx-ink-muted);
    }
    .qr {
      position: relative;
      display: flex;
      justify-content: center;
      margin: 0;
    }
    .qr__image {
      width: 200px;
      height: 200px;
      background-color: #fff;
      border-radius: var(--phx-radius-medium);
    }
    .qr__modules {
      fill: #000;
    }
    .qr__copy {
      position: absolute;
      right: 0;
      bottom: 0;
    }
    .codes {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 4px 16px;
      padding: 0 0 0 24px;
      margin: 0;
      font: var(--phx-mono-14);
    }
    @media (max-width: 640px) {
      .codes {
        grid-template-columns: minmax(0, 1fr);
      }
    }
  `,
  host: {
    '[attr.data-stage]': 'stage().kind',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelTotpEnrollComponent {
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly ref = inject(PhxModalRef);
  private readonly held = signal<EnrollStage>(CONFIRM_STAGE);
  private readonly working = signal(false);
  private readonly refusal = signal<string | null>(null);

  readonly enrolled: Enrolled = enrolledOf(this.ref.data);
  readonly form = codeForm();
  readonly stage = this.held.asReadonly();
  readonly busy = this.working.asReadonly();
  readonly error = this.refusal.asReadonly();
  readonly qr = computed(() => qrOf(this.enrolled.uri));
  readonly codes = computed(() => {
    const stage = this.held();
    return stage.kind === 'recovery' ? stage.codes : [];
  });

  protected readonly viewBox = computed(() => viewBoxOf(this.qr()));
  protected readonly codesText = computed(() => this.codes().join('\n'));
  protected readonly secretLabel = computed(() => this.i18n.t(SECURITY_SECRET_KEY));
  protected readonly secretHint = computed(() => this.i18n.t(SECURITY_SECRET_HINT_KEY));
  protected readonly codeLabel = computed(() => this.i18n.t(CODE_LABEL_KEY));
  protected readonly confirmLabel = computed(() => this.i18n.t(SECURITY_CONFIRM_KEY));
  protected readonly recoveryLabel = computed(() => this.i18n.t(SECURITY_RECOVERY_KEY));
  protected readonly recoveryHint = computed(() => this.i18n.t(SECURITY_RECOVERY_HINT_KEY));
  protected readonly finishLabel = computed(() => this.i18n.t(FINISH_KEY));

  async confirm(): Promise<void> {
    this.form.markAllAsTouched();
    if (this.busy() || this.form.invalid) {
      return;
    }
    this.working.set(true);
    this.refusal.set(null);
    try {
      const envelope = await firstValueFrom(
        this.client.call(TOTP_CONFIRM, this.form.getRawValue()),
      ).catch((error: unknown): Envelope => ({ ok: false, error: unreachable(error) }));
      if (envelope.ok) {
        this.held.set({ kind: 'recovery', codes: recoveryCodesOf(envelope.data) });
      } else {
        this.refusal.set(codeErrorOf(this.i18n, envelope.error));
      }
    } finally {
      this.working.set(false);
    }
  }

  finish(): void {
    this.ref.close(true);
  }
}

function enrolledOf(data: unknown): Enrolled {
  if (!isEnrolled(data)) {
    throw new TypeError('the enrolment modal was opened without the secret and the uri');
  }
  return data;
}
