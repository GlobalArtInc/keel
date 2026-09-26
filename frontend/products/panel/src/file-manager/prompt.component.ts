import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule } from '@angular/forms';
import {
  PHX_MODAL_DATA,
  PhxButtonComponent,
  PhxFormButtonsComponent,
  PhxInputComponent,
  PhxModalRef,
} from '@globalart/phoenix-ui';

export interface PromptData {
  label: string;
  value: string;
  submit: string;
  cancel: string;
  invalid: string;
  valid: (value: string) => boolean;
}

@Component({
  selector: 'panel-file-prompt',
  imports: [ReactiveFormsModule, PhxButtonComponent, PhxFormButtonsComponent, PhxInputComponent],
  template: `
    <form class="form" novalidate [formGroup]="form" (ngSubmit)="submit()">
      <phx-input
        width="full"
        autocomplete="off"
        formControlName="value"
        [label]="data.label"
        [error]="error()"
      />
      <phx-form-buttons align="end">
        <button phxButton type="button" class="cancel" (click)="cancel()">
          {{ data.cancel }}
        </button>
        <button phxButton="primary" type="submit" class="submit">{{ data.submit }}</button>
      </phx-form-buttons>
    </form>
  `,
  styles: `
    :host {
      display: block;
    }
    .form {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class FilePromptComponent {
  private readonly ref = inject<PhxModalRef<string, PromptData>>(PhxModalRef);
  private readonly refusal = signal<string | null>(null);

  readonly data = inject<PromptData>(PHX_MODAL_DATA);
  readonly form = new FormGroup({
    value: new FormControl(this.data.value, { nonNullable: true }),
  });
  readonly error = this.refusal.asReadonly();

  submit(): void {
    const value = this.form.controls.value.value.trim();
    if (!this.data.valid(value)) {
      this.refusal.set(this.data.invalid);
      return;
    }
    this.ref.close(value);
  }

  cancel(): void {
    this.ref.close();
  }
}
