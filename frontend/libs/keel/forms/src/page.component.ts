import { ChangeDetectionStrategy, Component, input } from '@angular/core';
import { KeelFieldComponent } from './field.component';
import type { FormPage } from './model';

@Component({
  selector: 'keel-page',
  imports: [KeelFieldComponent],
  template: `
    @for (field of page().fields; track field.name) {
      @if (field.visible()) {
        <keel-field [field]="field" />
      }
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: var(--phx-form-row-gap, 28px);
      max-width: var(--keel-form-control-max, 640px);
    }
  `,
  host: {
    '[attr.data-page]': 'page().id',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelPageComponent {
  readonly page = input.required<FormPage>();
}
