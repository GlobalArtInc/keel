export type JsonObject = Record<string, unknown>;

export interface ResolvedSchema {
  schema: JsonObject;
  nullable: boolean;
  type: string | null;
}

const MAX_DEPTH = 8;
const DEFS_PREFIX = '#/$defs/';

export function isObject(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function defsOf(schema: unknown): JsonObject {
  return isObject(schema) && isObject(schema['$defs']) ? schema['$defs'] : {};
}

export function propertiesOf(schema: unknown): JsonObject {
  return isObject(schema) && isObject(schema['properties']) ? schema['properties'] : {};
}

export function requiredOf(schema: unknown): string[] {
  const required = isObject(schema) ? schema['required'] : undefined;
  return Array.isArray(required)
    ? required.filter((name): name is string => typeof name === 'string')
    : [];
}

export function resolveSchema(property: unknown, defs: JsonObject, depth = 0): ResolvedSchema {
  if (!isObject(property) || depth > MAX_DEPTH) {
    return { schema: {}, nullable: false, type: null };
  }
  const { $ref: reference, anyOf, oneOf, ...local } = property;
  let merged: JsonObject = {};
  let nullable = false;
  if (typeof reference === 'string' && reference.startsWith(DEFS_PREFIX)) {
    const target = resolveSchema(defs[reference.slice(DEFS_PREFIX.length)], defs, depth + 1);
    merged = target.schema;
    nullable = target.nullable;
  }
  const branches = Array.isArray(anyOf) ? anyOf : Array.isArray(oneOf) ? oneOf : null;
  if (branches !== null) {
    const live = branches.filter((branch) => !isNullSchema(branch));
    nullable = nullable || live.length < branches.length;
    if (live.length === 1) {
      const target = resolveSchema(live[0], defs, depth + 1);
      merged = { ...merged, ...target.schema };
      nullable = nullable || target.nullable;
    }
  }
  const schema = { ...merged, ...local };
  const declared = schema['type'];
  let type: string | null = typeof declared === 'string' ? declared : null;
  if (Array.isArray(declared)) {
    const types = declared.filter((entry): entry is string => typeof entry === 'string');
    nullable = nullable || types.includes('null');
    const concrete = types.filter((entry) => entry !== 'null');
    type = concrete.length === 1 ? concrete[0] : null;
  }
  if (type === 'null') {
    return { schema, nullable: true, type: null };
  }
  return { schema, nullable, type };
}

export function enumOf(schema: JsonObject): string[] | null {
  const listed = schema['enum'];
  if (!Array.isArray(listed)) {
    return null;
  }
  return listed.filter((value): value is string => typeof value === 'string');
}

export function numberOf(schema: JsonObject, keyword: string): number | null {
  const value = schema[keyword];
  return typeof value === 'number' ? value : null;
}

function isNullSchema(branch: unknown): boolean {
  return isObject(branch) && branch['type'] === 'null';
}
