import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { RouterLink } from '@angular/router';
import { KeelAuth, KeelBoot, KeelI18n } from '@globalart/keel/client';
import type { CallError, Envelope } from '@globalart/keel/meta';
import {
  PhxBannerComponent,
  PhxButtonComponent,
  PhxFullscreenLayoutComponent,
  PhxInputComponent,
  PhxLogonComponent,
  PhxPasswordComponent,
  PhxTextButtonComponent,
} from '@globalart/phoenix-ui';
import { LOGIN_PATH } from '../guard';
import { BRAND_KEY } from './login.component';

export const RESET_TITLE_KEY = 'keel.shell.reset.title';
export const RESET_INTRO_KEY = 'keel.shell.reset.intro';
export const RESET_LOGIN_KEY = 'keel.shell.reset.login';
export const RESET_SUBMIT_KEY = 'keel.shell.reset.submit';
export const RESET_SENT_KEY = 'keel.shell.reset.sent';
export const RESET_PASSWORD_KEY = 'keel.shell.reset.password';
export const RESET_SAVE_KEY = 'keel.shell.reset.save';
export const RESET_DONE_KEY = 'keel.shell.reset.done';
export const RESET_FAILED_KEY = 'keel.shell.reset.failed';
export const RESET_TO_LOGIN_KEY = 'keel.shell.reset.to_login';

export type ResetStage = 'request' | 'sent' | 'confirm' | 'done';

export interface ResetNotice {
  message: string;
}

function required(): FormControl<string> {
  return new FormControl('', { nonNullable: true, validators: [Validators.required] });
}

export function refusal(i18n: KeelI18n, error: CallError): string {
  const field = error.fields?.find((entry) => typeof entry.key === 'string');
  return typeof field?.key === 'string'
    ? i18n.t(field.key, field.params)
    : i18n.t(error.key, error.params);
}

@Component({
  selector: 'keel-reset',
  imports: [
    ReactiveFormsModule,
    RouterLink,
    PhxFullscreenLayoutComponent,
    PhxLogonComponent,
    PhxInputComponent,
    PhxPasswordComponent,
    PhxButtonComponent,
    PhxTextButtonComponent,
    PhxBannerComponent,
  ],
  template: `
    <phx-fullscreen-layout>
      <span phxLogo class="brand">{{ brand() }}</span>
      <phx-logon [title]="title()">
        @switch (stage()) {
          @case ('sent') {
            <phx-banner status="info" role="status">{{ sentText() }}</phx-banner>
          }
          @case ('done') {
            <phx-banner status="success" role="status">{{ doneText() }}</phx-banner>
          }
          @case ('confirm') {
            <form
              class="form"
              data-step="confirm"
              novalidate
              [formGroup]="confirmForm"
              (ngSubmit)="confirm()"
            >
              @if (notice(); as notice) {
                <phx-banner status="danger" role="alert">{{ notice.message }}</phx-banner>
              }
              <phx-password
                formControlName="password"
                width="full"
                autocomplete="new-password"
                [label]="passwordLabel()"
              />
              <button
                phxButton="primary"
                type="submit"
                class="submit"
                [loading]="busy()"
                [disabled]="busy()"
              >
                {{ saveLabel() }}
              </button>
            </form>
          }
          @default {
            <form
              class="form"
              data-step="request"
              novalidate
              [formGroup]="requestForm"
              (ngSubmit)="request()"
            >
              <p class="intro">{{ intro() }}</p>
              @if (notice(); as notice) {
                <phx-banner status="danger" role="alert">{{ notice.message }}</phx-banner>
              }
              <phx-input
                formControlName="login"
                width="full"
                autocomplete="username"
                [label]="loginLabel()"
              />
              <button
                phxButton="primary"
                type="submit"
                class="submit"
                [loading]="busy()"
                [disabled]="busy()"
              >
                {{ submitLabel() }}
              </button>
            </form>
          }
        }
        <a phxLogonFooter phxTextButton="link" class="to-login" [routerLink]="loginPath">
          {{ toLoginLabel() }}
        </a>
      </phx-logon>
    </phx-fullscreen-layout>
  `,
  styles: `
    :host {
      display: block;
    }
    .brand {
      font: var(--phx-heading-25);
      color: var(--phx-ink);
    }
    .form {
      display: flex;
      flex-direction: column;
      gap: 28px;
    }
    .intro {
      margin: 0;
      color: var(--phx-ink-muted);
      text-align: center;
    }
    .submit {
      --phx-button-width: 100%;
    }
  `,
  host: {
    '[attr.data-stage]': 'stage()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelResetComponent {
  private readonly auth = inject(KeelAuth);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly working = signal(false);
  private readonly finished = signal<'sent' | 'done' | null>(null);
  private readonly shown = signal<ResetNotice | null>(null);

  readonly token = input<string | undefined>();
  readonly requestForm = new FormGroup({ login: required() });
  readonly confirmForm = new FormGroup({ password: required() });
  readonly loginPath = `/${LOGIN_PATH}`;

  readonly busy = this.working.asReadonly();
  readonly notice = this.shown.asReadonly();
  readonly stage = computed<ResetStage>(() => {
    const token = this.token();
    return this.finished() ?? (token !== undefined && token !== '' ? 'confirm' : 'request');
  });

  protected readonly brand = computed(() => this.i18n.t(this.boot.brand()?.name ?? BRAND_KEY));
  protected readonly title = computed(() => this.i18n.t(RESET_TITLE_KEY));
  protected readonly intro = computed(() => this.i18n.t(RESET_INTRO_KEY));
  protected readonly loginLabel = computed(() => this.i18n.t(RESET_LOGIN_KEY));
  protected readonly submitLabel = computed(() => this.i18n.t(RESET_SUBMIT_KEY));
  protected readonly sentText = computed(() => this.i18n.t(RESET_SENT_KEY));
  protected readonly passwordLabel = computed(() => this.i18n.t(RESET_PASSWORD_KEY));
  protected readonly saveLabel = computed(() => this.i18n.t(RESET_SAVE_KEY));
  protected readonly doneText = computed(() => this.i18n.t(RESET_DONE_KEY));
  protected readonly toLoginLabel = computed(() => this.i18n.t(RESET_TO_LOGIN_KEY));

  async request(): Promise<void> {
    this.requestForm.markAllAsTouched();
    if (this.busy() || this.requestForm.invalid) {
      return;
    }
    const { login } = this.requestForm.getRawValue();
    await this.perform(this.auth.requestReset({ login: login.trim() }), 'sent');
  }

  async confirm(): Promise<void> {
    const token = this.token();
    this.confirmForm.markAllAsTouched();
    if (this.busy() || this.confirmForm.invalid || token === undefined || token === '') {
      return;
    }
    const { password } = this.confirmForm.getRawValue();
    await this.perform(this.auth.confirmReset({ token, password }), 'done');
  }

  private async perform(call: Promise<Envelope<unknown>>, next: 'sent' | 'done'): Promise<void> {
    this.working.set(true);
    this.shown.set(null);
    try {
      const envelope = await call;
      if (envelope.ok) {
        this.finished.set(next);
      } else {
        this.shown.set({ message: refusal(this.i18n, envelope.error) });
      }
    } catch {
      this.shown.set({ message: this.i18n.t(RESET_FAILED_KEY) });
    } finally {
      this.working.set(false);
    }
  }
}
