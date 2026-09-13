import type { CallError, FieldError } from '@globalart/keel/meta';
import type { FormField, KeelForm } from './model';
import { FINDINGS } from './validators';

export type FieldFallback = Pick<CallError, 'code' | 'key' | 'params'>;

const ROOT = /^[^.[]+/;

export function rootOf(path: string): string {
  return ROOT.exec(path)?.[0] ?? '';
}

export function fieldOf(form: KeelForm, path: string): FormField | undefined {
  return form.field(rootOf(path));
}

export function applyFieldErrors(
  form: KeelForm,
  fields: readonly FieldError[],
  fallback?: FieldFallback,
): FieldError[] {
  const mapped = new Map<FormField, FieldError[]>();
  const unmapped: FieldError[] = [];
  for (const entry of fields) {
    const finding = fallback === undefined ? entry : filled(entry, fallback);
    const field = fieldOf(form, entry.path);
    if (field === undefined || !field.visible()) {
      unmapped.push(finding);
      continue;
    }
    const findings = mapped.get(field);
    if (findings === undefined) {
      mapped.set(field, [finding]);
    } else {
      findings.push(finding);
    }
  }
  for (const [field, findings] of mapped) {
    field.control.setErrors({ [FINDINGS]: findings });
    field.control.markAsTouched();
  }
  form.unmapped.set(unmapped);
  return unmapped;
}

export function applyCallError(form: KeelForm, error: CallError): FieldError[] {
  const fields = error.fields ?? [];
  if (fields.length === 0) {
    return applyFieldErrors(form, [filled({ path: '' }, error)]);
  }
  return applyFieldErrors(form, fields, error);
}

export function clearFieldErrors(form: KeelForm): void {
  form.unmapped.set([]);
}

function filled(entry: FieldError, fallback: FieldFallback): FieldError {
  if (entry.code || entry.key) {
    return entry;
  }
  const finding: FieldError = { ...entry, code: fallback.code, key: fallback.key };
  if (entry.params === undefined && fallback.params !== undefined) {
    finding.params = fallback.params;
  }
  return finding;
}
