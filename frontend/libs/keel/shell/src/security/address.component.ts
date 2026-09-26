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
  isObject,
  unreachable,
  type KeelForm,
} from '@globalart/keel/forms';
import type { CallError, Descriptor, Envelope, ScreenPage } from '@globalart/keel/meta';
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
  PhxPanelComponent,
  PhxToastService,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';

export const ADDRESS_GET = 'auth.address.get';
export const ADDRESS_SET = 'auth.address.set';
export const ADDRESS_FIELDS: readonly string[] = ['value', 'current'];
export const SECURITY_ADDRESS_KEY = 'keel.shell.security.address';
export const SECURITY_ADDRESS_NONE_KEY = 'keel.shell.security.address_none';
export const SECURITY_ADDRESS_VERIFIED_KEY = 'keel.shell.security.address_verified';
export const SECURITY_ADDRESS_PENDING_KEY = 'keel.shell.security.address_pending';
export const SECURITY_ADDRESS_SENT_KEY = 'keel.shell.security.address_sent';

export interface AddressView {
  principal: string;
  value: string | null;
  verified: boolean;
  verifiedAt: string | null;
}

export type AddressState =
  | { phase: 'loading' }
  | { phase: 'ready'; form: KeelForm; address: AddressView }
  | { phase: 'failed'; error: CallError };

export function ownAddressDescriptor(descriptor: Descriptor): Descriptor {
  const pages: ScreenPage[] = (descriptor.ui?.pages ?? []).map((page) => ({
    ...page,
    fields: (page.fields ?? []).filter((field) => ADDRESS_FIELDS.includes(field)),
  }));
  return {
    ...descriptor,
    ui: {
      ...(descriptor.ui ?? { title: descriptor.name }),
      pages: pages.length > 0 ? pages : [{ id: 'main', fields: [...ADDRESS_FIELDS] }],
    },
  };
}

function isAddressView(value: unknown): value is AddressView {
  return isObject(value) && typeof value['verified'] === 'boolean';
}

function addressOf(envelope: Envelope<AddressView>): AddressView {
  if (!envelope.ok) {
    throw new KeelCallError(envelope.error);
  }
  return envelope.data;
}

@Component({
  selector: 'keel-security-address',
  imports: [
    PhxPanelComponent,
    PhxLoaderComponent,
    PhxErrorComponent,
    PhxButtonComponent,
    PhxFormButtonsComponent,
    KeelFindingsComponent,
    KeelPageComponent,
  ],
  template: `
    <phx-panel data-section="address" [title]="label()" [description]="status()">
      @switch (state().phase) {
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
  `,
  styles: `
    :host {
      display: block;
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
    '[attr.data-address]': 'state().phase',
    '[attr.data-verified]': 'verified()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelSecurityAddressComponent {
  private readonly context = injectKeelViewContext();
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly toasts = inject(PhxToastService);
  private readonly widgets = injectKeelWidgets();
  private readonly held = signal<AddressState>({ phase: 'loading' });
  private sequence = 0;

  readonly state = this.held.asReadonly();
  readonly verified = computed(() => {
    const state = this.held();
    return state.phase === 'ready' && state.address.verified;
  });

  protected readonly form = computed(() => {
    const state = this.held();
    if (state.phase !== 'ready') {
      throw new TypeError('the address form is not ready');
    }
    return state.form;
  });
  protected readonly label = computed(() => this.i18n.t(SECURITY_ADDRESS_KEY));
  protected readonly status = computed(() => {
    const state = this.held();
    if (state.phase !== 'ready' || state.address.value === null) {
      return this.i18n.t(SECURITY_ADDRESS_NONE_KEY);
    }
    const key = state.address.verified
      ? SECURITY_ADDRESS_VERIFIED_KEY
      : SECURITY_ADDRESS_PENDING_KEY;
    return this.i18n.t(key, { value: state.address.value });
  });
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
    this.fetch().then(
      ([descriptor, address]) => {
        if (started !== this.sequence) {
          return;
        }
        const form = buildForm(ownAddressDescriptor(descriptor), {
          mode: 'create',
          client: this.context.client,
          principal: this.boot.principal(),
          widgets: this.widgets,
          values: address.value === null ? {} : { value: address.value },
        });
        this.held.set({ phase: 'ready', form, address });
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

  private async fetch(): Promise<[Descriptor, AddressView]> {
    return Promise.all([
      this.descriptors.get(ADDRESS_SET),
      firstValueFrom(this.context.client.call<AddressView>(ADDRESS_GET, {})).then(addressOf),
    ]);
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
    const saved = isAddressView(envelope.data) ? envelope.data : null;
    this.toasts.success(
      saved === null || saved.verified
        ? this.i18n.t(DONE_KEY, { action: this.i18n.t(state.form.title) })
        : this.i18n.t(SECURITY_ADDRESS_SENT_KEY, { value: saved.value ?? '' }),
    );
    this.load();
  }

  private drop(): void {
    const state = this.held();
    if (state.phase === 'ready') {
      state.form.destroy();
    }
  }
}
