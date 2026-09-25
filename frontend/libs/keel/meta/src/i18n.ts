import type { Column, Descriptor } from './generated';
import { isObject, type JsonObject } from './json';

export const SHARED_PREFIX = 'keel';

const MAX_DEPTH = 8;
const DEFS_PREFIX = '#/$defs/';
const PLACEHOLDER = /\{([A-Za-z_][A-Za-z0-9_]*)\}/g;

export interface DerivedKey {
  member: string;
  key: string;
  shared: string;
}

export type Messages = Record<string, unknown>;

export function derivedKeys(descriptor: Descriptor): DerivedKey[] {
  const ui = descriptor.ui;
  if (!ui) {
    return [];
  }
  const keys = new Keys(entityPrefix(descriptor.name));
  const params = isObject(descriptor.params) ? descriptor.params : {};
  const properties = isObject(params['properties']) ? params['properties'] : undefined;
  const defs = isObject(params['$defs']) ? params['$defs'] : {};
  for (const field of fields(descriptor, properties)) {
    keys.push(`field ${field}`, field);
    const schema = properties?.[field];
    if (schema === undefined) {
      continue;
    }
    for (const value of enumValues(schema, defs, 0)) {
      keys.push(`field ${field} value ${value}`, `${field}.${value}`);
    }
  }
  for (const page of ui.pages ?? []) {
    keys.push(`page ${page.id}`, `page.${page.id}`);
  }
  for (const action of ui.actions ?? []) {
    keys.push(`action ${action.id}`, `action.${action.id}`);
  }
  for (const column of ui.columns ?? []) {
    pushColumn(keys, column);
  }
  for (const band of ui.bands ?? []) {
    keys.push(`band ${band.id}`, `band.${band.id}`);
    for (const column of band.columns) {
      pushColumn(keys, column);
    }
  }
  for (const chart of ui.charts ?? []) {
    keys.push(`chart ${chart.id}`, `chart.${chart.id}`);
  }
  return keys.derived;
}

function pushColumn(keys: Keys, column: Column): void {
  keys.push(`column ${column.key}`, `column.${column.key}`);
  const values: string[] = [];
  if (column.filter && 'options' in column.filter) {
    values.push(...column.filter.options);
  }
  if (column.map) {
    values.push(...Object.keys(column.map).sort());
  }
  for (const value of values) {
    keys.push(`column ${column.key} value ${value}`, `${column.key}.${value}`);
  }
}

export function entityPrefix(func: string): string {
  const dot = func.lastIndexOf('.');
  return dot < 0 ? func : func.slice(0, dot);
}

const PREFIX_DEPTHS: readonly number[] = [2, 1];

export function sharedKeys(key: string): string[] {
  const segments = key.split('.');
  return PREFIX_DEPTHS.filter((depth) => segments.length > depth)
    .map((depth) => `${SHARED_PREFIX}.${segments.slice(depth).join('.')}`)
    .filter((shared) => shared !== key);
}

export function sharedKey(key: string): string | undefined {
  return sharedKeys(key)[0];
}

export function lookup(catalogue: Messages, key: string): string | undefined {
  const own = find(catalogue, key);
  if (typeof own === 'string') {
    return own;
  }
  for (const shared of sharedKeys(key)) {
    const inherited = find(catalogue, shared);
    if (typeof inherited === 'string') {
      return inherited;
    }
  }
  return undefined;
}

export function format(message: string, params: Record<string, unknown> = {}): string {
  return message.replace(PLACEHOLDER, (placeholder: string, name: string) => {
    if (!Object.hasOwn(params, name)) {
      return placeholder;
    }
    return render(params[name]) ?? placeholder;
  });
}

function render(value: unknown): string | undefined {
  switch (typeof value) {
    case 'string':
      return value;
    case 'number':
    case 'boolean':
    case 'bigint':
      return String(value);
    case 'undefined':
      return undefined;
    default:
      return JSON.stringify(value);
  }
}

function find(messages: JsonObject, key: string): unknown {
  if (Object.hasOwn(messages, key)) {
    return messages[key];
  }
  for (let index = key.indexOf('.'); index >= 0; index = key.indexOf('.', index + 1)) {
    const head = key.slice(0, index);
    const child = Object.hasOwn(messages, head) ? messages[head] : undefined;
    if (isObject(child)) {
      const found = find(child, key.slice(index + 1));
      if (found !== undefined) {
        return found;
      }
    }
  }
  return undefined;
}

class Keys {
  readonly derived: DerivedKey[] = [];
  private readonly seen = new Set<string>();

  constructor(private readonly prefix: string) {}

  push(member: string, tail: string): void {
    const key = `${this.prefix}.${tail}`;
    if (this.seen.has(key)) {
      return;
    }
    this.seen.add(key);
    this.derived.push({ member, key, shared: `${SHARED_PREFIX}.${tail}` });
  }
}

function fields(descriptor: Descriptor, properties: JsonObject | undefined): string[] {
  const paged = (descriptor.ui?.pages ?? []).flatMap((page) => page.fields ?? []);
  const fromPages = paged.length > 0;
  const candidates = fromPages ? paged : Object.keys(properties ?? {});
  const listed: string[] = [];
  for (const field of candidates) {
    if (!fromPages && descriptor.key === field) {
      continue;
    }
    if (!listed.includes(field)) {
      listed.push(field);
    }
  }
  return listed;
}

function enumValues(schema: unknown, defs: JsonObject, depth: number): string[] {
  const values: string[] = [];
  if (depth > MAX_DEPTH || !isObject(schema)) {
    return values;
  }
  const listed = schema['enum'];
  if (Array.isArray(listed)) {
    values.push(...listed.filter((value): value is string => typeof value === 'string'));
  }
  const reference = schema['$ref'];
  if (typeof reference === 'string' && reference.startsWith(DEFS_PREFIX)) {
    values.push(...enumValues(defs[reference.slice(DEFS_PREFIX.length)], defs, depth + 1));
  }
  for (const combinator of ['anyOf', 'oneOf']) {
    const branches = schema[combinator];
    if (Array.isArray(branches)) {
      for (const branch of branches) {
        values.push(...enumValues(branch, defs, depth + 1));
      }
    }
  }
  if (schema['items'] !== undefined) {
    values.push(...enumValues(schema['items'], defs, depth + 1));
  }
  return values.filter((value, index) => values.indexOf(value) === index);
}
