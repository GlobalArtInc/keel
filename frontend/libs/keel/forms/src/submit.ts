import type { KeelClient, UploadFiles } from '@globalart/keel/client';
import type { CallError, Envelope } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { applyCallError, clearFieldErrors } from './errors';
import type { FormField, KeelForm } from './model';
import { isEmptyValue, isFileList } from './validators';

export type Saved<T = unknown> = Extract<Envelope<T>, { ok: true }>;

export function isFileField(field: FormField): boolean {
  return field.mapping.component === 'phx-file-field';
}

export function isUpload(form: KeelForm): boolean {
  return form.descriptor.kind === 'upload';
}

export function bodyOf(
  form: KeelForm,
  fields: readonly FormField[] = form.fields,
): Record<string, unknown> {
  const raw = form.group.getRawValue();
  const body: Record<string, unknown> = {};
  if (form.key !== null && !isEmptyValue(raw[form.key])) {
    body[form.key] = raw[form.key];
  }
  const upload = isUpload(form);
  for (const field of fields) {
    if (!field.visible() || (upload && isFileField(field))) {
      continue;
    }
    const value = raw[field.name];
    const blank = isEmptyValue(value) && !field.required() && field.nullable;
    body[field.name] = blank ? null : (value ?? null);
  }
  return body;
}

export function filesOf(form: KeelForm, fields: readonly FormField[] = form.fields): UploadFiles {
  const files: Record<string, File[]> = {};
  for (const field of fields) {
    if (!field.visible() || !isFileField(field)) {
      continue;
    }
    const value = field.control.value;
    files[field.name] = isFileList(value) ? value : [];
  }
  return files;
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
    isUpload(form)
      ? client.upload<T>(form.name, bodyOf(form), filesOf(form), { idempotencyKey })
      : client.call<T>(form.name, bodyOf(form), { idempotencyKey }),
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
