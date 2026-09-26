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
  type KeelForm,
} from '@globalart/keel/forms';
import type { CallError, Envelope } from '@globalart/keel/meta';
import {
  DONE_KEY,
  RETRY_KEY,
  describeError,
  injectKeelViewContext,
} from '@globalart/keel/renderer';
import {
  PhxButtonComponent,
  PhxErrorComponent,
  PhxFormButtonsComponent,
  PhxLoaderComponent,
  PhxModalService,
  PhxPageHeaderComponent,
  PhxPanelComponent,
  PhxToastService,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { KeelSecurityAddressComponent } from './address.component';
import { KeelTotpDisableComponent } from './disable.component';
import { KeelTotpEnrollComponent } from './enroll.component';
import {
  DISABLE_TITLE_KEY,
  ENROLL_TITLE_KEY,
  PASSWORD_SET,
  SECURITY_DISABLE_KEY,
  SECURITY_ENROLLED_KEY,
  SECURITY_ENROLL_KEY,
  SECURITY_NOT_ENROLLED_KEY,
  SECURITY_PASSWORD_HINT_KEY,
  SECURITY_PASSWORD_KEY,
  SECURITY_TITLE_KEY,
  SECURITY_TOTP_KEY,
  TOTP_ENROLL,
  ownPasswordDescriptor,
  type Enrolled,
} from './totp';

export type PasswordState =
  { phase: 'loading' } | { phase: 'ready'; form: KeelForm } | { phase: 'failed'; error: CallError };

export const MODAL_WIDTH = 400;

@Component({
  selector: 'keel-security',
  imports: [
    PhxPageHeaderComponent,
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxButtonComponent,
    PhxFormButtonsComponent,
    KeelFindingsComponent,
    KeelPageComponent,
    KeelSecurityAddressComponent,
  ],
  template: `
    <phx-page-header [title]="title()" />
    <phx-panel data-section="password" [title]="passwordLabel()" [description]="passwordHint()">
      @switch (password().phase) {
        @case ('loading') {
          <div class="loading"><phx-loader [size]="24" /></div>
        }
        @case ('failed') {
          <phx-error [text]="message()" [retryLabel]="retryLabel()" (retry)="load()" />
        }
        @case ('ready') {
          <form class="form" novalidate (submit)="save($event)">
            @if (form().unmapped().length) {
              <keel-findings [findings]="form().unmapped()" />
            }
            @for (page of form().visiblePages(); track page.id) {
              <keel-page [page]="page" />
            }
            <phx-form-buttons align="end">
              <button
                phxButton="primary"
                type="submit"
                class="save"
                [loading]="form().pending()"
                [disabled]="form().pending()"
              >
                {{ saveLabel() }}
              </button>
            </phx-form-buttons>
          </form>
        }
      }
    </phx-panel>
    <keel-security-address />
    <phx-panel
      data-section="totp"
      [title]="totpLabel()"
      [description]="totpState()"
      [attr.data-enrolled]="enrolled()"
    >
      <phx-form-buttons>
        @if (enrolled()) {
          <button phxButton type="button" class="disable" (click)="disable()">
            {{ disableLabel() }}
          </button>
        } @else {
          <button
            phxButton="primary"
            type="button"
            class="enroll"
            [loading]="enrolling()"
            [disabled]="enrolling()"
            (click)="enroll()"
          >
            {{ enrollLabel() }}
          </button>
        }
      </phx-form-buttons>
    </phx-panel>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
      min-height: 0;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    .form {
      display: flex;
      flex-direction: column;
      gap: var(--phx-form-row-gap, 28px);
    }
  `,
  host: {
    '[attr.data-security]': 'func',
    '[attr.data-password]': 'password().phase',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelSecurityComponent {
  readonly context = injectKeelViewContext();
  readonly func = this.context.descriptor?.name ?? TOTP_ENROLL;

  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly modals = inject(PhxModalService);
  private readonly toasts = inject(PhxToastService);
  private readonly widgets = injectKeelWidgets();
  private readonly held = signal<PasswordState>({ phase: 'loading' });
  private readonly working = signal(false);
  private sequence = 0;

  readonly password = this.held.asReadonly();
  readonly enrolling = this.working.asReadonly();
  readonly enrolled = computed(() => this.boot.principal()?.totp === true);

  protected readonly form = computed(() => {
    const state = this.held();
    if (state.phase !== 'ready') {
      throw new TypeError('the password form is not ready');
    }
    return state.form;
  });
  protected readonly title = computed(() => this.i18n.t(SECURITY_TITLE_KEY));
  protected readonly passwordLabel = computed(() => this.i18n.t(SECURITY_PASSWORD_KEY));
  protected readonly passwordHint = computed(() => this.i18n.t(SECURITY_PASSWORD_HINT_KEY));
  protected readonly totpLabel = computed(() => this.i18n.t(SECURITY_TOTP_KEY));
  protected readonly totpState = computed(() =>
    this.i18n.t(this.enrolled() ? SECURITY_ENROLLED_KEY : SECURITY_NOT_ENROLLED_KEY),
  );
  protected readonly enrollLabel = computed(() => this.i18n.t(SECURITY_ENROLL_KEY));
  protected readonly disableLabel = computed(() => this.i18n.t(SECURITY_DISABLE_KEY));
  protected readonly saveLabel = computed(() => this.i18n.t(SAVE_KEY));
  protected readonly retryLabel = computed(() => this.i18n.t(RETRY_KEY));
  protected readonly message = computed(() => {
    const state = this.held();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });

  constructor() {
    this.load();
    inject(DestroyRef).onDestroy(() => this.drop());
  }

  load(): void {
    const started = ++this.sequence;
    this.drop();
    this.held.set({ phase: 'loading' });
    this.descriptors.get(PASSWORD_SET).then(
      (descriptor) => {
        if (started !== this.sequence) {
          return;
        }
        const form = buildForm(ownPasswordDescriptor(descriptor), {
          mode: 'create',
          client: this.context.client,
          principal: this.boot.principal(),
          widgets: this.widgets,
        });
        this.held.set({ phase: 'ready', form });
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

  async save(event?: Event): Promise<void> {
    event?.preventDefault();
    const state = this.held();
    if (state.phase !== 'ready' || state.form.pending()) {
      return;
    }
    const envelope = await state.form.submit();
    if (envelope === null || !envelope.ok) {
      return;
    }
    this.toasts.success(this.i18n.t(DONE_KEY, { action: this.i18n.t(state.form.title) }));
    this.load();
  }

  async enroll(): Promise<void> {
    if (this.working()) {
      return;
    }
    this.working.set(true);
    try {
      const envelope = await firstValueFrom(
        this.context.client.call<Enrolled>(TOTP_ENROLL, {}),
      ).catch((error: unknown): Envelope<Enrolled> => ({ ok: false, error: unreachable(error) }));
      if (!envelope.ok) {
        this.toasts.danger(describeError(this.i18n, envelope.error));
        return;
      }
      const ref = this.modals.open(KeelTotpEnrollComponent, {
        title: this.i18n.t(ENROLL_TITLE_KEY),
        width: MODAL_WIDTH,
        data: envelope.data,
      });
      ref.afterClosed.subscribe(() => void this.refresh());
    } finally {
      this.working.set(false);
    }
  }

  disable(): void {
    const ref = this.modals.open(KeelTotpDisableComponent, {
      title: this.i18n.t(DISABLE_TITLE_KEY),
      width: MODAL_WIDTH,
    });
    ref.afterClosed.subscribe((disabled) => {
      if (disabled === true) {
        void this.refresh();
      }
    });
  }

  private async refresh(): Promise<void> {
    try {
      await this.boot.refresh();
    } catch (error: unknown) {
      this.toasts.danger(describeError(this.i18n, unreachable(error)));
    }
  }

  private drop(): void {
    const state = this.held();
    if (state.phase === 'ready') {
      state.form.destroy();
    }
  }
}
