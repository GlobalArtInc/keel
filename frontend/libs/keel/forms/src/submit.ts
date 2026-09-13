import type { KeelClient } from '@globalart/keel/client';
import type { CallError, Envelope } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { applyCallError, clearFieldErrors } from './errors';
import type { KeelForm } from './model';
import { isEmptyValue } from './validators';

export type Saved<T = unknown> = Extract<Envelope<T>, { ok: true }>;

export function bodyOf(form: KeelForm): Record<string, unknown> {
  const raw = form.group.getRawValue();
  const body: Record<string, unknown> = {};
  if (form.key !== null && !isEmptyValue(raw[form.key])) {
    body[form.key] = raw[form.key];
  }
  for (const field of form.fields) {
    if (!field.visible()) {
      continue;
    }
    const value = raw[field.name];
    body[field.name] = isEmptyValue(value) && !field.required() ? null : (value ?? null);
  }
  return body;
}

// https://www.rfc-editor.org/rfc/rfc9562#section-5.4
export function newIdempotencyKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20),
  ].join('-');
}

export function markAllTouched(form: KeelForm): void {
  for (const field of form.fields) {
    field.control.markAsTouched();
  }
}

export async function submitForm<T = unknown>(
  form: KeelForm,
  client: KeelClient,
  idempotencyKey: string,
): Promise<Envelope<T> | null> {
  clearFieldErrors(form);
  markAllTouched(form);
  if (form.group.invalid) {
    return null;
  }
  const envelope = await firstValueFrom(
    client.call<T>(form.name, bodyOf(form), { idempotencyKey }),
  ).catch((error: unknown): Envelope<T> => ({ ok: false, error: unreachable(error) }));
  if (!envelope.ok) {
    applyCallError(form, envelope.error);
  }
  return envelope;
}

export function unreachable(error: unknown): CallError {
  return {
    code: 'unavailable',
    key: 'keel.error.unavailable',
    params: { reason: error instanceof Error ? error.message : String(error) },
  };
}
