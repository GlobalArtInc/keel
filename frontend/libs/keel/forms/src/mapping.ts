import type { Check, Control, KeelAttrs, UiAttrs } from '@globalart/keel/meta';
import { enumOf, isObject, numberOf, propertiesOf, resolveSchema, type JsonObject } from './schema';

export interface FieldSchema {
  name: string;
  schema: JsonObject;
  nullable: boolean;
  type: string | null;
  keel: KeelAttrs;
  ui: UiAttrs;
  defs: JsonObject;
}

export type InputType = 'text' | 'email' | 'url';

export type ControlMapping = { preset: Check | null } & (
  | { component: 'phx-input'; type: InputType }
  | { component: 'phx-password'; generate: boolean }
  | { component: 'phx-select'; choices: string[]; remote: string | null }
  | { component: 'phx-toggle' }
  | { component: 'phx-switcher'; choices: string[] }
  | { component: 'phx-slider'; min: number; max: number; step: number; unlimited: boolean }
  | {
      component: 'phx-number-field';
      min: number | null;
      max: number | null;
      step: number;
      unlimited: boolean;
    }
  | { component: 'phx-date-field' }
  | { component: 'phx-time-field' }
  | { component: 'phx-file-field'; accept: string[]; maxSize: number | null }
  | { component: 'phx-list-field'; columns: string[] }
  | { component: 'phx-code-editor' }
  | { component: 'phx-tags-field' }
  | { component: 'phx-key-value-field'; keyPattern: string | null }
  | { component: 'phx-transfer-list'; choices: string[]; remote: string | null }
);

export type PhxControl = ControlMapping['component'];

export const CONTROLS: readonly Control[] = [
  'input',
  'password',
  'select',
  'toggle',
  'switcher',
  'slider',
  'date',
  'time',
  'file',
  'list',
  'code',
  'tags',
  'key_value',
  'transfer',
];

export function mapControl(field: FieldSchema): ControlMapping {
  const preset = field.keel.check ?? null;
  const control = field.keel.secret ? 'password' : (field.ui.control ?? inferControl(field));
  switch (control) {
    case 'input':
      return { component: 'phx-input', type: inputType(field.schema), preset };
    case 'password':
      return { component: 'phx-password', generate: field.ui.generate === true, preset };
    case 'select':
      return {
        component: 'phx-select',
        choices: field.keel.options ? [] : (enumOf(field.schema) ?? []),
        remote: field.keel.options ?? null,
        preset,
      };
    case 'toggle':
      return { component: 'phx-toggle', preset };
    case 'switcher':
      return { component: 'phx-switcher', choices: enumOf(field.schema) ?? [], preset };
    case 'slider':
      return {
        component: 'phx-slider',
        min: numberOf(field.schema, 'minimum') ?? 0,
        max: numberOf(field.schema, 'maximum') ?? 100,
        step: 1,
        unlimited: field.nullable,
        preset,
      };
    case 'date':
      return { component: 'phx-date-field', preset };
    case 'time':
      return { component: 'phx-time-field', preset };
    case 'file':
      return {
        component: 'phx-file-field',
        accept: field.keel.file?.types ?? [],
        maxSize: field.keel.file?.maxBytes ?? null,
        preset,
      };
    case 'list':
      return { component: 'phx-list-field', columns: columnsOf(field), preset };
    case 'code':
      return { component: 'phx-code-editor', preset };
    case 'tags':
      return { component: 'phx-tags-field', preset };
    case 'key_value':
      return { component: 'phx-key-value-field', keyPattern: keyPatternOf(field.schema), preset };
    case 'transfer':
      return {
        component: 'phx-transfer-list',
        choices: field.keel.options ? [] : (enumOf(itemsOf(field).schema) ?? []),
        remote: field.keel.options ?? null,
        preset,
      };
    case 'number':
      return {
        component: 'phx-number-field',
        min: numberOf(field.schema, 'minimum'),
        max: numberOf(field.schema, 'maximum'),
        step: 1,
        unlimited: field.nullable,
        preset,
      };
  }
}

type Inferred = Control | 'number';

function inferControl(field: FieldSchema): Inferred {
  if (field.type === 'boolean') {
    return 'toggle';
  }
  if (field.keel.options || enumOf(field.schema) !== null) {
    return 'select';
  }
  if (field.type === 'integer' || field.type === 'number') {
    return 'number';
  }
  if (field.type === 'string' && field.schema['format'] === 'date') {
    return 'date';
  }
  if (field.type === 'array' && itemsOf(field).type === 'string') {
    return 'tags';
  }
  return 'input';
}

function inputType(schema: JsonObject): InputType {
  switch (schema['format']) {
    case 'email':
      return 'email';
    case 'uri':
      return 'url';
    default:
      return 'text';
  }
}

function keyPatternOf(schema: JsonObject): string | null {
  const names = schema['propertyNames'];
  const pattern = isObject(names) ? names['pattern'] : null;
  return typeof pattern === 'string' ? pattern : null;
}

function itemsOf(field: FieldSchema) {
  return resolveSchema(field.schema['items'], field.defs);
}

function columnsOf(field: FieldSchema): string[] {
  return Object.keys(propertiesOf(itemsOf(field).schema));
}
