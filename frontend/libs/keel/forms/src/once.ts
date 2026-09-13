import { entityPrefix, type Descriptor, type KeelAttrs } from '@globalart/keel/meta';
import { defsOf, isObject, propertiesOf, resolveSchema, type JsonObject } from './schema';

export interface OnceRow {
  path: string;
  labelKey: string;
  value: string;
}

export function onceOf(descriptor: Descriptor, data: unknown): OnceRow[] {
  const result = isObject(descriptor.result) ? descriptor.result : {};
  const rows: OnceRow[] = [];
  collect(result, defsOf(result), data, '', entityPrefix(descriptor.name), rows);
  return rows;
}

function collect(
  schema: JsonObject,
  defs: JsonObject,
  data: unknown,
  path: string,
  prefix: string,
  rows: OnceRow[],
): void {
  if (!isObject(data)) {
    return;
  }
  for (const [name, property] of Object.entries(propertiesOf(schema))) {
    const value = data[name];
    if (value === null || value === undefined) {
      continue;
    }
    const resolved = resolveSchema(property, defs);
    const attrs = resolved.schema['x-keel'];
    const nested = path === '' ? name : `${path}.${name}`;
    if (isObject(attrs) && (attrs as KeelAttrs).once === true) {
      rows.push({ path: nested, labelKey: `${prefix}.${nested}`, value: textOf(value) });
    } else if (resolved.type === 'object') {
      collect(resolved.schema, defs, value, nested, prefix, rows);
    }
  }
}

function textOf(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value);
}
