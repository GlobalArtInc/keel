import { FormControl, FormGroup, Validators } from '@angular/forms';
import type { KeelI18n } from '@globalart/keel/client';
import { rootOf } from '@globalart/keel/forms';
import type { CallError } from '@globalart/keel/meta';
import { describeError } from '@globalart/keel/renderer';

export const CODE_FIELD = 'code';

export type CodeForm = FormGroup<{ code: FormControl<string> }>;

export function codeForm(): CodeForm {
  return new FormGroup({
    code: new FormControl('', { nonNullable: true, validators: [Validators.required] }),
  });
}

export function codeErrorOf(i18n: KeelI18n, error: CallError): string {
  const own = (error.fields ?? []).find((field) => rootOf(field.path) === CODE_FIELD);
  if (own === undefined) {
    return describeError(i18n, error);
  }
  return i18n.t(own.key ?? error.key, own.params ?? error.params);
}
