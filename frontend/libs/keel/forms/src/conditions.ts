import { evaluate, type Context, type ContextPrincipal, type Expr } from '@globalart/keel/meta';
import type { FormField, FormMode, KeelForm } from './model';

export interface ConditionScope {
  mode: FormMode;
  principal: ContextPrincipal | null;
  params: Record<string, unknown>;
}

export function contextOf(fields: Record<string, unknown>, scope: ConditionScope): Context {
  return {
    fields,
    mode: scope.mode,
    principal: scope.principal,
    screen: { params: scope.params },
  };
}

export function holds(expr: Expr | null | undefined, context: Context, fallback: boolean): boolean {
  return expr ? evaluate(expr, context) : fallback;
}

const REQUIRED = new WeakMap<FormField, boolean>();

export function applyConditions(form: KeelForm): void {
  for (const field of form.fields) {
    const disabled = !field.visible() || field.readonly();
    if (field.control.disabled !== disabled) {
      if (disabled) {
        field.control.disable();
      } else {
        field.control.enable();
      }
    }
    const required = field.required();
    if (REQUIRED.get(field) !== required) {
      REQUIRED.set(field, required);
      field.control.updateValueAndValidity();
    }
  }
}
