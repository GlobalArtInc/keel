import {
  defsOf,
  isObject,
  propertiesOf,
  resolveSchema,
  type JsonObject,
} from '@globalart/keel/forms';
import { entityPrefix, type Descriptor } from '@globalart/keel/meta';

export type ObjectValue =
  | { kind: 'empty' }
  | { kind: 'text'; text: string }
  | { kind: 'enum'; value: string; labelKey: string }
  | { kind: 'boolean'; value: boolean }
  | { kind: 'datetime'; value: string }
  | { kind: 'json'; text: string };

export interface ObjectRow {
  path: string;
  labelKey: string;
  columnKey: string;
  value: ObjectValue;
}

const DATE_TIME = 'date-time';

export function objectRows(descriptor: Descriptor, data: unknown): ObjectRow[] {
  const result = isObject(descriptor.result) ? descriptor.result : {};
  const rows: ObjectRow[] = [];
  collect(result, defsOf(result), data, '', entityPrefix(descriptor.name), rows);
  return rows;
}

function collect(
  schema: JsonObject,
  defs: JsonObject,
  data: unknown,
  path: string,
  prefix: string,
  rows: ObjectRow[],
): void {
  const source = isObject(data) ? data : {};
  for (const [name, property] of Object.entries(propertiesOf(schema))) {
    const value = source[name];
    const nested = path === '' ? name : `${path}.${name}`;
    const resolved = resolveSchema(property, defs);
    const nestedProperties = propertiesOf(resolved.schema);
    if (resolved.type === 'object' && Object.keys(nestedProperties).length > 0) {
      if (value !== null && value !== undefined) {
        collect(resolved.schema, defs, value, nested, prefix, rows);
      }
      continue;
    }
    rows.push({
      path: nested,
      labelKey: `${prefix}.${nested}`,
      columnKey: `${prefix}.column.${nested}`,
      value: valueOf(resolved.schema, value, `${prefix}.${nested}`),
    });
  }
}

function valueOf(schema: JsonObject, value: unknown, labelPrefix: string): ObjectValue {
  if (value === null || value === undefined) {
    return { kind: 'empty' };
  }
  const listed = schema['enum'];
  if (typeof value === 'string' && Array.isArray(listed) && listed.includes(value)) {
    return { kind: 'enum', value, labelKey: `${labelPrefix}.${value}` };
  }
  if (typeof value === 'boolean') {
    return { kind: 'boolean', value };
  }
  if (typeof value === 'string') {
    const format = schema['format'];
    if (format === DATE_TIME) {
      return { kind: 'datetime', value };
    }
    return { kind: 'text', text: value };
  }
  if (typeof value === 'number' || typeof value === 'bigint') {
    return { kind: 'text', text: String(value) };
  }
  if (Array.isArray(value) && value.every(isScalar)) {
    return { kind: 'text', text: value.map(String).join(', ') };
  }
  return { kind: 'json', text: JSON.stringify(value) };
}

function isScalar(value: unknown): value is string | number | boolean {
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean';
}
