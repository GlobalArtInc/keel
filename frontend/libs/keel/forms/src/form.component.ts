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
import {
  PhxFormSectionComponent,
  PhxFormSheetComponent,
  PhxTabComponent,
  PhxTabGroupComponent,
} from '@globalart/phoenix-ui';
import type { FormPage, KeelForm } from './model';
import { KeelPageComponent } from './page.component';

@Component({
  selector: 'keel-form',
  imports: [
    PhxFormSheetComponent,
    PhxFormSectionComponent,
    PhxTabGroupComponent,
    PhxTabComponent,
    KeelPageComponent,
  ],
  template: `
    <phx-form-sheet [title]="title()" [closable]="closable()" (closed)="closed.emit()">
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

  protected readonly selected = signal(0);

  private readonly i18n = inject(KeelI18n);

  protected readonly title = computed(() => this.i18n.t(this.form().title));
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
  }

  protected labelOf(page: FormPage): string {
    return this.labels().get(page.id) ?? page.labelKey;
  }
}
