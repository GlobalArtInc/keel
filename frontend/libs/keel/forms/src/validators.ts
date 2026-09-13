import type { AbstractControl, ValidatorFn } from '@angular/forms';
import { validate, type FieldError } from '@globalart/keel/meta';
import type { JsonObject } from './schema';

export const FINDINGS = 'keel';

export const REQUIRED_CODE = 'schema.required';
export const REQUIRED_KEY = 'keel.schema.required';

export interface FieldRule {
  name: string;
  schema: JsonObject;
  defs: JsonObject;
  required: () => boolean;
}

export function isEmptyValue(value: unknown): boolean {
  return (
    value === null || value === undefined || value === '' || (Array.isArray(value) && !value.length)
  );
}

export function validateField(rule: FieldRule, value: unknown): FieldError[] {
  if (isEmptyValue(value)) {
    return rule.required() ? [requiredFinding(rule.name)] : [];
  }
  const wrapper = {
    type: 'object',
    properties: { [rule.name]: rule.schema },
    $defs: rule.defs,
  };
  return validate(wrapper, { [rule.name]: value });
}

export function fieldValidator(rule: FieldRule): ValidatorFn {
  return (control) => {
    const findings = validateField(rule, control.value);
    return findings.length ? { [FINDINGS]: findings } : null;
  };
}

export function findingsOf(control: AbstractControl | null | undefined): FieldError[] {
  const findings = control?.errors?.[FINDINGS];
  return Array.isArray(findings) ? (findings as FieldError[]) : [];
}

function requiredFinding(name: string): FieldError {
  return { path: name, code: REQUIRED_CODE, key: REQUIRED_KEY, params: { property: name } };
}
