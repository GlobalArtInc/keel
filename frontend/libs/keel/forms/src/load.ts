import { KeelCallError, type KeelClient } from '@globalart/keel/client';
import type { Descriptor } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { buildForm, type FormOptions, type KeelForm } from './model';
import { isObject } from './schema';
import { isEmptyValue } from './validators';

export type LoadOptions = Omit<FormOptions, 'values' | 'client'> & { client: KeelClient };

export async function loadValues(
  descriptor: Descriptor,
  params: Record<string, unknown>,
  client: KeelClient,
): Promise<Record<string, unknown>> {
  const read = descriptor.ui?.read ?? null;
  const key = descriptor.key ?? null;
  if (read === null || key === null || isEmptyValue(params[key])) {
    return {};
  }
  const envelope = await firstValueFrom(client.call(read, { [key]: params[key] }));
  if (!envelope.ok) {
    throw new KeelCallError(envelope.error);
  }
  return isObject(envelope.data) ? envelope.data : {};
}

export async function loadForm(descriptor: Descriptor, options: LoadOptions): Promise<KeelForm> {
  const params = options.params ?? {};
  const values = await loadValues(descriptor, params, options.client);
  return buildForm(descriptor, { ...options, params, values });
}
