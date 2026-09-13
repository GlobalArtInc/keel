import type { KeelClient } from '@globalart/keel/client';
import type { Choice, Envelope, FieldError } from '@globalart/keel/meta';
import { Subscription, catchError, debounceTime, filter, of, switchMap } from 'rxjs';
import { applyCallError, applyFieldErrors } from './errors';
import type { FormField, KeelForm } from './model';
import { bodyOf, unreachable } from './submit';

export const RECOMPUTE_DEBOUNCE_MS = 300;

export interface RecomputeRequest {
  values: Record<string, unknown>;
  changed: string;
}

export interface RecomputeAnswer {
  values?: Record<string, unknown> | null;
  options?: Record<string, Choice[]> | null;
  errors?: FieldError[] | null;
}

export function recomputeFields(form: KeelForm): FormField[] {
  return form.fields.filter((field) => typeof field.conditions.recompute === 'string');
}

export function bindRecompute(
  form: KeelForm,
  client: KeelClient,
  debounce = RECOMPUTE_DEBOUNCE_MS,
): Subscription {
  const teardown = new Subscription();
  let applying = false;
  for (const field of recomputeFields(form)) {
    const func = field.conditions.recompute;
    if (typeof func !== 'string') {
      continue;
    }
    teardown.add(
      field.control.valueChanges
        .pipe(
          filter(() => !applying),
          debounceTime(debounce),
          switchMap(() =>
            client
              .call<RecomputeAnswer>(func, {
                values: bodyOf(form),
                changed: field.name,
              } satisfies RecomputeRequest)
              .pipe(
                catchError((error: unknown) =>
                  of<Envelope<RecomputeAnswer>>({ ok: false, error: unreachable(error) }),
                ),
              ),
          ),
        )
        .subscribe((envelope) => {
          if (!envelope.ok) {
            applyCallError(form, envelope.error);
            return;
          }
          applying = true;
          try {
            applyAnswer(form, envelope.data);
          } finally {
            applying = false;
          }
        }),
    );
  }
  return teardown;
}

export function applyAnswer(form: KeelForm, answer: RecomputeAnswer): void {
  if (answer.values) {
    form.group.patchValue(answer.values);
  }
  for (const [name, choices] of Object.entries(answer.options ?? {})) {
    const field = form.field(name);
    if (field === undefined) {
      continue;
    }
    if (field.options !== null) {
      field.options.replace(choices);
    } else {
      field.choices.set(choices);
    }
  }
  if (answer.errors && answer.errors.length > 0) {
    applyFieldErrors(form, answer.errors);
  }
}
