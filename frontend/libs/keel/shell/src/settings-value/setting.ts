import { defsOf, isObject, propertiesOf, type JsonObject } from '@globalart/keel/forms';
import type { Descriptor, Id, Level } from '@globalart/keel/meta';

export const SETTINGS_SET = 'settings.set';
export const SETTINGS_GET = 'settings.get';
export const KEY_FIELD = 'key';
export const VALUE_FIELD = 'value';
export const SETTINGS_TITLE_KEY = 'keel.shell.settings.title';
export const SETTINGS_DEFAULT_KEY = 'keel.shell.settings.default';

export interface SettingRow {
  key: string;
  module: string;
  schema: unknown;
  default?: unknown;
  level: Level;
  value?: unknown;
  scope?: Id | null;
}

const STRIPPED: readonly string[] = ['$schema', '$defs', '$id'];

export function isSettingRow(value: unknown): value is SettingRow {
  return (
    isObject(value) &&
    typeof value['key'] === 'string' &&
    typeof value['module'] === 'string' &&
    typeof value['level'] === 'string' &&
    Object.hasOwn(value, 'schema')
  );
}

export function settingKeyOf(params: Record<string, unknown>): string | null {
  const key = params[KEY_FIELD];
  return typeof key === 'string' && key !== '' ? key : null;
}

export function valueSchema(schema: unknown): JsonObject {
  if (!isObject(schema)) {
    return {};
  }
  return Object.fromEntries(Object.entries(schema).filter(([name]) => !STRIPPED.includes(name)));
}

export function valueDescriptor(descriptor: Descriptor, row: SettingRow): Descriptor {
  const params = isObject(descriptor.params) ? descriptor.params : {};
  const defs = { ...defsOf(params), ...defsOf(row.schema) };
  const next: JsonObject = {
    ...params,
    properties: { ...propertiesOf(params), [VALUE_FIELD]: valueSchema(row.schema) },
  };
  if (Object.keys(defs).length > 0) {
    next['$defs'] = defs;
  }
  return { ...descriptor, params: next };
}

export function valuesOf(row: SettingRow): Record<string, unknown> {
  return {
    [KEY_FIELD]: row.key,
    [VALUE_FIELD]: row.value === undefined ? row.default : row.value,
  };
}
