import { ChangeDetectionStrategy, Component, computed, inject, input, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { Router, RouterLink } from '@angular/router';
import { KeelAuth, KeelBoot, KeelI18n } from '@globalart/keel/client';
import type { CallError } from '@globalart/keel/meta';
import {
  PhxBannerComponent,
  PhxButtonComponent,
  PhxFullscreenLayoutComponent,
  PhxInputComponent,
  PhxLogonComponent,
  PhxPasswordComponent,
  PhxTextButtonComponent,
} from '@globalart/phoenix-ui';
import { HOME_URL, LOGIN_PATH } from '../guard';
import { BRAND_KEY } from './login.component';

export const SETUP_TITLE_KEY = 'keel.shell.setup.title';
export const SETUP_INTRO_KEY = 'keel.shell.setup.intro';
export const SETUP_LOGIN_KEY = 'keel.shell.setup.login';
export const SETUP_PASSWORD_KEY = 'keel.shell.setup.password';
export const SETUP_SUBMIT_KEY = 'keel.shell.setup.submit';
export const SETUP_FAILED_KEY = 'keel.shell.setup.failed';
export const SETUP_NO_KEY_KEY = 'keel.shell.setup.no_key';
export const SETUP_DONE_KEY = 'keel.shell.setup.done';
export const SETUP_SIGNING_IN_KEY = 'keel.shell.setup.signing_in';
export const SETUP_TO_LOGIN_KEY = 'keel.shell.setup.to_login';

export type SetupStage = 'form' | 'signing-in' | 'done';

export interface SetupNotice {
  title: string | null;
  message: string;
}

function required(): FormControl<string> {
  return new FormControl('', { nonNullable: true, validators: [Validators.required] });
}

@Component({
  selector: 'keel-setup',
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
    <phx-fullscreen-layout [loading]="stage() === 'signing-in'">
      <span phxLogo class="brand">{{ brand() }}</span>
      <phx-logon [title]="title()">
        @if (stage() === 'signing-in') {
          <p class="status" role="status">{{ signingInText() }}</p>
        } @else if (stage() === 'done') {
          <phx-banner status="info" role="status">{{ doneText() }}</phx-banner>
        } @else if (!hasKey()) {
          <phx-banner status="danger" role="alert">{{ noKeyText() }}</phx-banner>
        } @else {
          <form class="form" data-step="setup" novalidate [formGroup]="form" (ngSubmit)="create()">
            <p class="intro">{{ intro() }}</p>
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
              {{ submitLabel() }}
            </button>
          </form>
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
    .intro,
    .status {
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
export class KeelSetupComponent {
  private readonly auth = inject(KeelAuth);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);
  private readonly working = signal(false);
  private readonly held = signal<SetupStage>('form');
  private readonly shown = signal<SetupNotice | null>(null);

  readonly key = input<string | undefined>();
  readonly form = new FormGroup({ login: required(), password: required() });
  readonly loginPath = `/${LOGIN_PATH}`;

  readonly busy = this.working.asReadonly();
  readonly stage = this.held.asReadonly();
  readonly notice = this.shown.asReadonly();
  readonly hasKey = computed(() => {
    const key = this.key();
    return key !== undefined && key !== '';
  });

  protected readonly brand = computed(() => this.i18n.t(this.boot.brand()?.name ?? BRAND_KEY));
  protected readonly title = computed(() => this.i18n.t(SETUP_TITLE_KEY));
  protected readonly intro = computed(() => this.i18n.t(SETUP_INTRO_KEY));
  protected readonly loginLabel = computed(() => this.i18n.t(SETUP_LOGIN_KEY));
  protected readonly passwordLabel = computed(() => this.i18n.t(SETUP_PASSWORD_KEY));
  protected readonly submitLabel = computed(() => this.i18n.t(SETUP_SUBMIT_KEY));
  protected readonly noKeyText = computed(() => this.i18n.t(SETUP_NO_KEY_KEY));
  protected readonly doneText = computed(() => this.i18n.t(SETUP_DONE_KEY));
  protected readonly signingInText = computed(() => this.i18n.t(SETUP_SIGNING_IN_KEY));
  protected readonly toLoginLabel = computed(() => this.i18n.t(SETUP_TO_LOGIN_KEY));

  async create(): Promise<void> {
    const key = this.key();
    this.form.markAllAsTouched();
    if (this.busy() || this.form.invalid || key === undefined || key === '') {
      return;
    }
    const { login, password } = this.form.getRawValue();
    this.working.set(true);
    this.shown.set(null);
    try {
      const created = await this.auth.setupAdmin({ key, login, password });
      if (!created.ok) {
        this.refuse(created.error);
        return;
      }
      this.held.set('signing-in');
      const session = await this.auth.login({ login, password });
      if (session.ok && session.data.state === 'authenticated') {
        await this.router.navigateByUrl(HOME_URL);
      } else {
        await this.router.navigateByUrl(this.loginPath);
      }
    } catch {
      this.held.set('form');
      this.shown.set({ title: null, message: this.i18n.t(SETUP_FAILED_KEY) });
    } finally {
      this.working.set(false);
    }
  }

  private refuse(error: CallError): void {
    if (error.code === 'not_found') {
      this.held.set('done');
      return;
    }
    const field = error.fields?.find((entry) => typeof entry.key === 'string');
    const message =
      typeof field?.key === 'string'
        ? this.i18n.t(field.key, field.params)
        : this.i18n.t(error.key, error.params);
    this.shown.set({ title: this.i18n.t(SETUP_FAILED_KEY), message });
  }
}
