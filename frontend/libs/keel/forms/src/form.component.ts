import {
  ChangeDetectionStrategy,
  Component,
  booleanAttribute,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
} from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import type { FieldError } from '@globalart/keel/meta';
import {
  PhxBannerComponent,
  PhxButtonComponent,
  PhxFormFooterComponent,
  PhxFormSectionComponent,
  PhxFormSheetComponent,
  PhxTabComponent,
  PhxTabGroupComponent,
} from '@globalart/phoenix-ui';
import type { FormPage, KeelForm } from './model';
import { onceOf, type OnceRow } from './once';
import { KeelOnceComponent } from './once.component';
import { KeelPageComponent } from './page.component';
import type { Saved } from './submit';

export const SAVE_KEY = 'keel.action.save';
export const VALIDATION_KEY = 'keel.error.validation';

export interface BannerEntry {
  path: string;
  message: string;
}

@Component({
  selector: 'keel-form',
  imports: [
    PhxFormSheetComponent,
    PhxFormSectionComponent,
    PhxFormFooterComponent,
    PhxBannerComponent,
    PhxButtonComponent,
    PhxTabGroupComponent,
    PhxTabComponent,
    KeelPageComponent,
    KeelOnceComponent,
  ],
  template: `
    <phx-form-sheet
      [title]="title()"
      [closable]="closable()"
      [loading]="form().pending()"
      (closed)="closed.emit()"
      (submitted)="submit()"
    >
      @if (banner().length) {
        <phx-banner class="banner" status="danger" role="alert">
          @for (entry of banner(); track $index) {
            <div class="banner__entry">
              @if (entry.path) {
                <code class="banner__path">{{ entry.path }}</code>
              }
              <span class="banner__message">{{ entry.message }}</span>
            </div>
          }
        </phx-banner>
      }
      @if (once().length) {
        <keel-once class="once" [rows]="once()" />
      }
      @if (form().layout() === 'tabs') {
        <phx-tab-group [label]="title()" [(selectedIndex)]="selected">
          @for (page of form().visiblePages(); track page.id) {
            <phx-tab [label]="labelOf(page)">
              <keel-page [page]="page" />
            </phx-tab>
          }
        </phx-tab-group>
      } @else {
        @for (page of form().visiblePages(); track page.id) {
          <phx-form-section [title]="labelOf(page)">
            <keel-page [page]="page" />
          </phx-form-section>
        }
      }
      @if (submittable()) {
        <phx-form-footer>
          <button
            phxButton="primary"
            type="button"
            class="save"
            [loading]="form().pending()"
            [disabled]="form().pending()"
            (click)="submit()"
          >
            {{ saveLabel() }}
          </button>
        </phx-form-footer>
      }
    </phx-form-sheet>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    phx-form-sheet {
      flex: 1 1 auto;
      min-height: 0;
    }
    .banner,
    .once {
      margin-bottom: var(--phx-form-row-gap, 28px);
    }
    .banner__entry {
      display: flex;
      gap: 8px;
      align-items: baseline;
    }
    .banner__path {
      font: var(--phx-mono-14);
      color: var(--phx-ink-muted);
    }
  `,
  host: {
    '[attr.data-form]': 'form().name',
    '[attr.data-mode]': 'form().mode',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelFormComponent {
  readonly form = input.required<KeelForm>();
  readonly closable = input(true, { transform: booleanAttribute });

  readonly closed = output<void>();
  readonly submitted = output<Saved>();

  protected readonly selected = signal(0);
  protected readonly once = signal<readonly OnceRow[]>([]);

  private readonly i18n = inject(KeelI18n);

  protected readonly title = computed(() => this.i18n.t(this.form().title));
  protected readonly saveLabel = computed(() => this.i18n.t(SAVE_KEY));
  protected readonly submittable = computed(() => this.form().client !== null);
  protected readonly banner = computed<BannerEntry[]>(() =>
    this.form()
      .unmapped()
      .map((finding) => ({ path: finding.path, message: this.messageOf(finding) })),
  );
  private readonly labels = computed(
    () => new Map(this.form().pages.map((page) => [page.id, this.i18n.t(page.labelKey)])),
  );

  constructor() {
    effect(() => {
      const last = this.form().visiblePages().length - 1;
      if (this.selected() > last) {
        this.selected.set(Math.max(0, last));
      }
    });
    effect(() => {
      this.form();
      this.once.set([]);
    });
  }

  async submit(): Promise<void> {
    const form = this.form();
    if (form.client === null || form.pending()) {
      return;
    }
    const envelope = await form.submit();
    if (envelope === null || !envelope.ok) {
      return;
    }
    this.once.set(onceOf(form.descriptor, envelope.data));
    this.submitted.emit(envelope);
  }

  protected labelOf(page: FormPage): string {
    return this.labels().get(page.id) ?? page.labelKey;
  }

  private messageOf(finding: FieldError): string {
    return this.i18n.t(finding.key ?? finding.code ?? VALIDATION_KEY, finding.params);
  }
}
