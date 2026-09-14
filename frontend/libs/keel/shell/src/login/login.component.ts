import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { KeelAuth, KeelBoot, KeelI18n, type LoggedIn } from '@globalart/keel/client';
import type { CallError, Envelope } from '@globalart/keel/meta';
import { BACK_KEY } from '@globalart/keel/renderer';
import {
  PhxBannerComponent,
  PhxButtonComponent,
  PhxFullscreenLayoutComponent,
  PhxInputComponent,
  PhxLogonComponent,
  PhxPasswordComponent,
  PhxTextButtonComponent,
} from '@globalart/phoenix-ui';
import { RETURN_TO, returnTarget } from '../guard';

export const KEY_PARAM = 'key';
export const BRAND_KEY = 'keel.brand';
export const LOGIN_TITLE_KEY = 'keel.shell.login.title';
export const LOGIN_LOGIN_KEY = 'keel.shell.login.login';
export const LOGIN_PASSWORD_KEY = 'keel.shell.login.password';
export const LOGIN_SUBMIT_KEY = 'keel.shell.login.submit';
export const LOGIN_CODE_KEY = 'keel.shell.login.code';
export const LOGIN_CODE_HINT_KEY = 'keel.shell.login.code_hint';
export const LOGIN_VERIFY_KEY = 'keel.shell.login.verify';
export const LOGIN_KEY_KEY = 'keel.shell.login.key';
export const LOGIN_KEY_FAILED_KEY = 'keel.shell.login.key_failed';
export const LOGIN_FAILED_KEY = 'keel.shell.login.failed';

export type LoginStep = { kind: 'password' } | { kind: 'code'; challenge: string };

export interface LoginNotice {
  title: string | null;
  message: string;
}

const PASSWORD_STEP: LoginStep = { kind: 'password' };

function required(): FormControl<string> {
  return new FormControl('', { nonNullable: true, validators: [Validators.required] });
}

@Component({
  selector: 'keel-login',
  imports: [
    ReactiveFormsModule,
    PhxFullscreenLayoutComponent,
    PhxLogonComponent,
    PhxInputComponent,
    PhxPasswordComponent,
    PhxButtonComponent,
    PhxTextButtonComponent,
    PhxBannerComponent,
  ],
  template: `
    <phx-fullscreen-layout [loading]="redeeming()">
      <span phxLogo class="brand">{{ brand() }}</span>
      <phx-logon [title]="title()">
        @if (redeeming()) {
          <p class="redeeming" role="status">{{ redeemingText() }}</p>
        } @else if (step().kind === 'code') {
          <form
            class="form"
            data-step="code"
            novalidate
            [formGroup]="codeForm"
            (ngSubmit)="verify()"
          >
            @if (notice(); as notice) {
              <phx-banner status="danger" role="alert" [title]="notice.title">
                {{ notice.message }}
              </phx-banner>
            }
            <phx-input
              formControlName="code"
              width="full"
              autocomplete="one-time-code"
              [label]="codeLabel()"
            >
              <span phxFooter class="hint">{{ codeHint() }}</span>
            </phx-input>
            <button
              phxButton="primary"
              type="submit"
              class="submit"
              [loading]="busy()"
              [disabled]="busy()"
            >
              {{ verifyLabel() }}
            </button>
          </form>
        } @else {
          <form
            class="form"
            data-step="password"
            novalidate
            [formGroup]="passwordForm"
            (ngSubmit)="login()"
          >
            @if (notice(); as notice) {
              <phx-banner status="danger" role="alert" [title]="notice.title">
                {{ notice.message }}
              </phx-banner>
            }
            <phx-input
              formControlName="login"
              width="full"
              autocomplete="username"
              [label]="loginLabel()"
            />
            <phx-password
              formControlName="password"
              width="full"
              autocomplete="current-password"
              [label]="passwordLabel()"
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
        @if (step().kind === 'code') {
          <button
            phxLogonFooter
            phxTextButton="link"
            type="button"
            class="back"
            (click)="restart()"
          >
            {{ backLabel() }}
          </button>
        }
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
    .submit {
      --phx-button-width: 100%;
    }
    .redeeming {
      margin: 0;
      color: var(--phx-ink-muted);
      text-align: center;
    }
    .hint {
      font: var(--phx-text-12);
      color: var(--phx-ink-muted);
    }
  `,
  host: {
    '[attr.data-step]': 'redeeming() ? "key" : step().kind',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelLoginComponent {
  private readonly auth = inject(KeelAuth);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);
  private readonly route = inject(ActivatedRoute);
  private readonly held = signal<LoginStep>(PASSWORD_STEP);
  private readonly working = signal(false);
  private readonly redeem = signal(false);
  private readonly shown = signal<LoginNotice | null>(null);

  readonly passwordForm = new FormGroup({ login: required(), password: required() });
  readonly codeForm = new FormGroup({ code: required() });

  readonly step = this.held.asReadonly();
  readonly busy = this.working.asReadonly();
  readonly redeeming = this.redeem.asReadonly();
  readonly notice = this.shown.asReadonly();

  protected readonly brand = computed(() => this.i18n.t(this.boot.brand()?.name ?? BRAND_KEY));
  protected readonly title = computed(() => this.i18n.t(LOGIN_TITLE_KEY));
  protected readonly loginLabel = computed(() => this.i18n.t(LOGIN_LOGIN_KEY));
  protected readonly passwordLabel = computed(() => this.i18n.t(LOGIN_PASSWORD_KEY));
  protected readonly submitLabel = computed(() => this.i18n.t(LOGIN_SUBMIT_KEY));
  protected readonly codeLabel = computed(() => this.i18n.t(LOGIN_CODE_KEY));
  protected readonly codeHint = computed(() => this.i18n.t(LOGIN_CODE_HINT_KEY));
  protected readonly verifyLabel = computed(() => this.i18n.t(LOGIN_VERIFY_KEY));
  protected readonly redeemingText = computed(() => this.i18n.t(LOGIN_KEY_KEY));
  protected readonly backLabel = computed(() => this.i18n.t(BACK_KEY));

  constructor() {
    const key = this.route.snapshot.queryParamMap.get(KEY_PARAM);
    if (key !== null && key !== '') {
      void this.redeemKey(key);
    }
  }

  returnTo(): string {
    return returnTarget(this.route.snapshot.queryParamMap.get(RETURN_TO));
  }

  async login(): Promise<void> {
    this.passwordForm.markAllAsTouched();
    if (this.busy() || this.passwordForm.invalid) {
      return;
    }
    await this.perform(this.auth.login(this.passwordForm.getRawValue()));
  }

  async verify(): Promise<void> {
    const step = this.step();
    this.codeForm.markAllAsTouched();
    if (this.busy() || this.codeForm.invalid || step.kind !== 'code') {
      return;
    }
    const { code } = this.codeForm.getRawValue();
    await this.perform(this.auth.verifyTotp({ challenge: step.challenge, code }));
  }

  restart(): void {
    this.held.set(PASSWORD_STEP);
    this.shown.set(null);
    this.codeForm.reset();
    this.passwordForm.controls.password.reset();
  }

  private async perform(call: Promise<Envelope<LoggedIn>>): Promise<void> {
    this.working.set(true);
    this.shown.set(null);
    try {
      const envelope = await call;
      if (!envelope.ok) {
        this.refuse(envelope.error);
      } else if (envelope.data.state === 'totp') {
        this.held.set({ kind: 'code', challenge: envelope.data.challenge });
      } else {
        await this.router.navigateByUrl(this.returnTo());
      }
    } catch {
      this.shown.set({ title: null, message: this.i18n.t(LOGIN_FAILED_KEY) });
    } finally {
      this.working.set(false);
    }
  }

  private async redeemKey(key: string): Promise<void> {
    this.redeem.set(true);
    try {
      const envelope = await this.auth.redeemKey({ key });
      if (envelope.ok) {
        await this.router.navigateByUrl(envelope.data.redirect);
        return;
      }
      this.refuse(envelope.error, LOGIN_KEY_FAILED_KEY);
    } catch {
      this.shown.set({ title: null, message: this.i18n.t(LOGIN_KEY_FAILED_KEY) });
    } finally {
      this.redeem.set(false);
    }
    await this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { [KEY_PARAM]: null },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }

  private refuse(error: CallError, titleKey?: string): void {
    this.shown.set({
      title: titleKey === undefined ? null : this.i18n.t(titleKey),
      message: this.i18n.t(error.key, error.params),
    });
  }
}
