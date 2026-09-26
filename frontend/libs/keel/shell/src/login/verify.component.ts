import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  input,
  signal,
} from '@angular/core';
import { RouterLink } from '@angular/router';
import { KeelAuth, KeelBoot, KeelI18n } from '@globalart/keel/client';
import {
  PhxBannerComponent,
  PhxFullscreenLayoutComponent,
  PhxLogonComponent,
  PhxTextButtonComponent,
} from '@globalart/phoenix-ui';
import { HOME_URL } from '../guard';
import { BRAND_KEY } from './login.component';
import { refusal } from './reset.component';

export const VERIFY_TITLE_KEY = 'keel.shell.verify.title';
export const VERIFY_PENDING_KEY = 'keel.shell.verify.pending';
export const VERIFY_DONE_KEY = 'keel.shell.verify.done';
export const VERIFY_FAILED_KEY = 'keel.shell.verify.failed';
export const VERIFY_CONTINUE_KEY = 'keel.shell.verify.continue';

export type VerifyState =
  { phase: 'pending' } | { phase: 'done'; value: string } | { phase: 'failed'; message: string };

@Component({
  selector: 'keel-verify',
  imports: [
    RouterLink,
    PhxFullscreenLayoutComponent,
    PhxLogonComponent,
    PhxBannerComponent,
    PhxTextButtonComponent,
  ],
  template: `
    <phx-fullscreen-layout [loading]="state().phase === 'pending'">
      <span phxLogo class="brand">{{ brand() }}</span>
      <phx-logon [title]="title()">
        @switch (state().phase) {
          @case ('done') {
            <phx-banner status="success" role="status">{{ message() }}</phx-banner>
          }
          @case ('failed') {
            <phx-banner status="danger" role="alert">{{ message() }}</phx-banner>
          }
          @default {
            <p class="pending" role="status">{{ message() }}</p>
          }
        }
        <a phxLogonFooter phxTextButton="link" class="continue" [routerLink]="homeUrl">
          {{ continueLabel() }}
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
    .pending {
      margin: 0;
      color: var(--phx-ink-muted);
      text-align: center;
    }
  `,
  host: {
    '[attr.data-phase]': 'state().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelVerifyComponent {
  private readonly auth = inject(KeelAuth);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly held = signal<VerifyState>({ phase: 'pending' });
  private sent: string | null = null;

  readonly token = input<string | undefined>();
  readonly state = this.held.asReadonly();
  readonly homeUrl = HOME_URL;

  protected readonly brand = computed(() => this.i18n.t(this.boot.brand()?.name ?? BRAND_KEY));
  protected readonly title = computed(() => this.i18n.t(VERIFY_TITLE_KEY));
  protected readonly continueLabel = computed(() => this.i18n.t(VERIFY_CONTINUE_KEY));
  protected readonly message = computed(() => {
    const state = this.held();
    switch (state.phase) {
      case 'done':
        return this.i18n.t(VERIFY_DONE_KEY, { value: state.value });
      case 'failed':
        return state.message;
      default:
        return this.i18n.t(VERIFY_PENDING_KEY);
    }
  });

  constructor() {
    effect(() => {
      const token = this.token() ?? '';
      if (token !== this.sent) {
        this.sent = token;
        void this.verify(token);
      }
    });
  }

  private async verify(token: string): Promise<void> {
    if (token === '') {
      this.held.set({ phase: 'failed', message: this.i18n.t(VERIFY_FAILED_KEY) });
      return;
    }
    this.held.set({ phase: 'pending' });
    try {
      const envelope = await this.auth.verifyAddress({ token });
      this.held.set(
        envelope.ok
          ? { phase: 'done', value: envelope.data.value }
          : { phase: 'failed', message: refusal(this.i18n, envelope.error) },
      );
    } catch {
      this.held.set({ phase: 'failed', message: this.i18n.t(VERIFY_FAILED_KEY) });
    }
  }
}
