import type { KeelClient } from '@globalart/keel/client';
import {
  applyCallError,
  bodyOf,
  clearFieldErrors,
  unreachable,
  type FormField,
  type FormPage,
  type KeelForm,
} from '@globalart/keel/forms';
import type { Envelope } from '@globalart/keel/meta';
import type { PhxWizardStep } from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';

export function stepsOf(
  pages: readonly FormPage[],
  reached: number,
  label: (page: FormPage) => string,
): PhxWizardStep[] {
  return pages.map((page, index) => ({
    id: page.id,
    label: label(page),
    disabled: index > reached,
  }));
}

export function fieldsUpTo(pages: readonly FormPage[], index: number): FormField[] {
  return pages.slice(0, index + 1).flatMap((page) => page.fields);
}

export function markPageTouched(page: FormPage): void {
  for (const field of page.fields) {
    field.control.markAsTouched();
  }
}

export function pageValid(page: FormPage): boolean {
  return page.fields.every((field) => !field.control.invalid);
}

export function firstInvalidPage(pages: readonly FormPage[]): number {
  return pages.findIndex((page) => !pageValid(page));
}

export async function validateStep(
  form: KeelForm,
  pages: readonly FormPage[],
  index: number,
  client: KeelClient,
): Promise<boolean> {
  const page = pages[index];
  clearFieldErrors(form);
  markPageTouched(page);
  if (!pageValid(page)) {
    return false;
  }
  if (page.validate === null) {
    return true;
  }
  const envelope = await firstValueFrom(
    client.call(page.validate, bodyOf(form, fieldsUpTo(pages, index))),
  ).catch((error: unknown): Envelope => ({ ok: false, error: unreachable(error) }));
  if (!envelope.ok) {
    applyCallError(form, envelope.error);
    return false;
  }
  return true;
}
