import { check, checkCode, checkKey, isCheck } from './check';
import { compilePattern } from './expr';
import type { Check, FieldError } from './generated';
import { equal, has, isObject, type JsonObject } from './json';

export type Schema = JsonObject | boolean;

const MAX_DEPTH = 8;
const DEFS_PREFIX = '#/$defs/';

interface Walk {
  defs: JsonObject;
  errors: FieldError[];
}

export function validate(schema: Schema, value: unknown): FieldError[] {
  const defs = isObject(schema) && isObject(schema['$defs']) ? schema['$defs'] : {};
  const errors: FieldError[] = [];
  visit(schema, value, '', { defs, errors });
  probeTree(treeOf(schema, defs, 0), value, '', errors);
  return errors;
}

export function pushSegment(path: string, name: string): string {
  return path.length === 0 ? name : `${path}.${name}`;
}

export function pushIndex(path: string, index: number): string {
  return `${path}[${index}]`;
}

function visit(schema: Schema, value: unknown, path: string, walk: Walk): void {
  if (schema === true) {
    return;
  }
  if (schema === false) {
    walk.errors.push(schemaError(path, 'schema'));
    return;
  }
  const type = schema['type'];
  if (typeof type === 'string' || Array.isArray(type)) {
    const accepted = (Array.isArray(type) ? type : [type]).some(
      (candidate) => typeof candidate === 'string' && hasType(value, candidate),
    );
    if (!accepted) {
      walk.errors.push(schemaError(path, 'type', { value }));
    }
  }
  const choices = schema['enum'];
  if (Array.isArray(choices) && !choices.some((choice) => equal(value, choice))) {
    walk.errors.push(schemaError(path, 'enum', { value }));
  }
  if (typeof value === 'number') {
    const minimum = schema['minimum'];
    if (typeof minimum === 'number' && value < minimum) {
      walk.errors.push(schemaError(path, 'minimum', { limit: minimum }));
    }
    const maximum = schema['maximum'];
    if (typeof maximum === 'number' && value > maximum) {
      walk.errors.push(schemaError(path, 'maximum', { limit: maximum }));
    }
  }
  if (typeof value === 'string') {
    const minLength = schema['minLength'];
    if (typeof minLength === 'number' && codePoints(value) < minLength) {
      walk.errors.push(schemaError(path, 'minLength', { limit: minLength }));
    }
    const maxLength = schema['maxLength'];
    if (typeof maxLength === 'number' && codePoints(value) > maxLength) {
      walk.errors.push(schemaError(path, 'maxLength', { limit: maxLength }));
    }
  }
  const required = schema['required'];
  if (isObject(value) && Array.isArray(required)) {
    for (const property of required) {
      if (typeof property === 'string' && !has(value, property)) {
        walk.errors.push(schemaError(pushSegment(path, property), 'required', { property }));
      }
    }
  }
  if (typeof value === 'string') {
    const pattern = schema['pattern'];
    if (typeof pattern === 'string' && !compilePattern(pattern).test(value)) {
      walk.errors.push(schemaError(path, 'pattern'));
    }
    const format = schema['format'];
    if (typeof format === 'string' && !hasFormat(value, format)) {
      walk.errors.push(schemaError(path, 'format'));
    }
  }
  const properties = schema['properties'];
  if (isObject(value) && isObject(properties)) {
    for (const [name, property] of Object.entries(properties)) {
      if (has(value, name) && isSchema(property)) {
        visit(property, value[name], pushSegment(path, name), walk);
      }
    }
  }
  const items = schema['items'];
  if (Array.isArray(value) && isSchema(items)) {
    value.forEach((item, index) => visit(items, item, pushIndex(path, index), walk));
  }
  const allOf = schema['allOf'];
  if (Array.isArray(allOf)) {
    for (const branch of allOf) {
      if (isSchema(branch)) {
        visit(branch, value, path, walk);
      }
    }
  }
  const anyOf = schema['anyOf'];
  if (Array.isArray(anyOf)) {
    const branches = tryBranches(anyOf, value, path, walk);
    if (!branches.some((branch) => branch.errors.length === 0)) {
      unfold('anyOf', branches, path, walk);
    }
  }
  const oneOf = schema['oneOf'];
  if (Array.isArray(oneOf)) {
    const branches = tryBranches(oneOf, value, path, walk);
    const valid = branches.filter((branch) => branch.errors.length === 0).length;
    if (valid === 0) {
      unfold('oneOf', branches, path, walk);
    } else if (valid > 1) {
      walk.errors.push(schemaError(path, 'oneOf'));
    }
  }
  const target = resolveRef(schema, walk.defs);
  if (target !== undefined) {
    visit(target, value, path, walk);
  }
}

interface Branch {
  schema: Schema;
  errors: FieldError[];
}

function tryBranches(candidates: unknown[], value: unknown, path: string, walk: Walk): Branch[] {
  const branches: Branch[] = [];
  for (const candidate of candidates) {
    if (!isSchema(candidate)) {
      continue;
    }
    const errors: FieldError[] = [];
    visit(candidate, value, path, { defs: walk.defs, errors });
    branches.push({ schema: candidate, errors });
  }
  return branches;
}

function unfold(keyword: string, branches: Branch[], path: string, walk: Walk): void {
  const live = branches.filter((branch) => !expectsNull(branch, walk.defs));
  if (live.length === 1) {
    walk.errors.push(...live[0].errors);
    return;
  }
  walk.errors.push(schemaError(path, keyword));
}

function expectsNull(branch: Branch, defs: JsonObject): boolean {
  if (branch.errors.length !== 1 || branch.errors[0].code !== 'schema.type') {
    return false;
  }
  const schema = isObject(branch.schema) ? (resolveRef(branch.schema, defs) ?? branch.schema) : {};
  return isObject(schema) && schema['type'] === 'null';
}

function resolveRef(schema: JsonObject, defs: JsonObject): Schema | undefined {
  const reference = schema['$ref'];
  if (typeof reference !== 'string' || !reference.startsWith(DEFS_PREFIX)) {
    return undefined;
  }
  const target = defs[reference.slice(DEFS_PREFIX.length)];
  return isSchema(target) ? target : undefined;
}

function isSchema(value: unknown): value is Schema {
  return typeof value === 'boolean' || isObject(value);
}

function schemaError(path: string, keyword: string, params?: JsonObject): FieldError {
  const error: FieldError = {
    path,
    code: `schema.${keyword}`,
    key: `keel.schema.${keyword}`,
  };
  if (params !== undefined) {
    error.params = params;
  }
  return error;
}

function hasType(value: unknown, type: string): boolean {
  switch (type) {
    case 'null':
      return value === null;
    case 'boolean':
      return typeof value === 'boolean';
    case 'number':
      return typeof value === 'number';
    case 'integer':
      return typeof value === 'number' && Number.isInteger(value);
    case 'string':
      return typeof value === 'string';
    case 'array':
      return Array.isArray(value);
    case 'object':
      return isObject(value);
    default:
      return false;
  }
}

function codePoints(text: string): number {
  return [...text].length;
}

function hasFormat(value: string, format: string): boolean {
  switch (format) {
    case 'date':
      return isDate(value);
    case 'date-time':
      return isDateTime(value);
    case 'email':
      return check('email', value);
    default:
      return true;
  }
}

function isDate(date: string): boolean {
  if (date.length !== 10 || date[4] !== '-' || date[7] !== '-') {
    return false;
  }
  const year = digits(date, 0, 4);
  const month = digits(date, 5, 7);
  const day = digits(date, 8, 10);
  if (year === undefined || month === undefined || day === undefined) {
    return false;
  }
  if (month < 1 || month > 12 || day === 0) {
    return false;
  }
  switch (month) {
    case 4:
    case 6:
    case 9:
    case 11:
      return day <= 30;
    case 2:
      return day <= (isLeapYear(year) ? 29 : 28);
    default:
      return day <= 31;
  }
}

function isLeapYear(year: number): boolean {
  return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
}

function isDateTime(text: string): boolean {
  const separator = text.search(/[Tt]/);
  if (separator < 0) {
    return false;
  }
  return isDate(text.slice(0, separator)) && isTime(text.slice(separator + 1));
}

function isTime(time: string): boolean {
  if (time.length < 9 || time[2] !== ':' || time[5] !== ':') {
    return false;
  }
  const hour = digits(time, 0, 2);
  const minute = digits(time, 3, 5);
  const second = digits(time, 6, 8);
  if (hour === undefined || minute === undefined || second === undefined) {
    return false;
  }
  if (hour > 23 || minute > 59 || second > 60) {
    return false;
  }
  let index = 8;
  if (time[index] === '.') {
    index += 1;
    let hasDigit = false;
    while (index < time.length && isAsciiDigit(time[index] ?? '')) {
      hasDigit = true;
      index += 1;
    }
    if (!hasDigit) {
      return false;
    }
  }
  if (index === time.length) {
    return false;
  }
  const marker = time[index];
  if (marker === 'Z' || marker === 'z') {
    return index === time.length - 1 && (second !== 60 || (hour === 23 && minute === 59));
  }
  if (marker !== '+' && marker !== '-') {
    return false;
  }
  if (time.length - index !== 6 || time[index + 3] !== ':') {
    return false;
  }
  const offsetHour = digits(time, index + 1, index + 3);
  const offsetMinute = digits(time, index + 4, index + 6);
  if (offsetHour === undefined || offsetMinute === undefined) {
    return false;
  }
  if (offsetHour > 23 || offsetMinute > 59) {
    return false;
  }
  if (second !== 60) {
    return true;
  }
  const sign = marker === '+' ? -1 : 1;
  let utcHour = hour + sign * offsetHour;
  let utcMinute = minute + sign * offsetMinute;
  utcHour += Math.trunc(utcMinute / 60);
  utcMinute %= 60;
  if (utcMinute < 0) {
    utcMinute += 60;
    utcHour -= 1;
  }
  utcHour = (utcHour + 24) % 24;
  return utcHour === 23 && utcMinute === 59;
}

function digits(text: string, start: number, end: number): number | undefined {
  const slice = text.slice(start, end);
  if (slice.length !== end - start || ![...slice].every(isAsciiDigit)) {
    return undefined;
  }
  return Number(slice);
}

function isAsciiDigit(char: string): boolean {
  return char >= '0' && char <= '9';
}

interface Tree {
  fields: Map<string, Node>;
  items?: Tree;
}

interface Node {
  check?: Check;
  checkBy?: CheckChoice;
  nested: Tree;
}

interface CheckChoice {
  field: string;
  cases: ReadonlyMap<string, Check>;
}

function emptyTree(): Tree {
  return { fields: new Map() };
}

function isEmptyTree(tree: Tree): boolean {
  return tree.fields.size === 0 && tree.items === undefined;
}

function treeOf(schema: Schema, defs: JsonObject, depth: number): Tree {
  if (depth > MAX_DEPTH || !isObject(schema)) {
    return emptyTree();
  }
  const resolved = typeof schema['$ref'] === 'string' ? resolveRef(schema, defs) : schema;
  if (!isObject(resolved)) {
    return emptyTree();
  }
  const properties = resolved['properties'];
  if (isObject(properties)) {
    const fields = new Map<string, Node>();
    for (const [name, property] of Object.entries(properties)) {
      if (!isSchema(property)) {
        continue;
      }
      fields.set(name, {
        check: presetOf(property),
        checkBy: choiceOf(property),
        nested: treeOf(property, defs, depth + 1),
      });
    }
    return { fields };
  }
  const items = resolved['items'];
  if (items !== undefined) {
    const nested = isSchema(items) ? treeOf(items, defs, depth + 1) : emptyTree();
    return isEmptyTree(nested) ? emptyTree() : { fields: new Map(), items: nested };
  }
  for (const combinator of ['anyOf', 'oneOf', 'allOf']) {
    const branches = resolved[combinator];
    if (!Array.isArray(branches)) {
      continue;
    }
    for (const branch of branches) {
      if (isSchema(branch)) {
        const nested = treeOf(branch, defs, depth + 1);
        if (!isEmptyTree(nested)) {
          return nested;
        }
      }
    }
  }
  return emptyTree();
}

function presetOf(property: Schema): Check | undefined {
  if (!isObject(property)) {
    return undefined;
  }
  const attrs = property['x-keel'];
  if (!isObject(attrs)) {
    return undefined;
  }
  const preset = attrs['check'];
  return isCheck(preset) ? preset : undefined;
}

function choiceOf(property: Schema): CheckChoice | undefined {
  const attrs = isObject(property) ? property['x-keel'] : undefined;
  const by = isObject(attrs) ? attrs['checkBy'] : undefined;
  if (!isObject(by) || typeof by['field'] !== 'string' || !isObject(by['cases'])) {
    return undefined;
  }
  const cases = new Map<string, Check>();
  for (const [value, preset] of Object.entries(by['cases'])) {
    if (isCheck(preset)) {
      cases.set(value, preset);
    }
  }
  return { field: by['field'], cases };
}

function presetsOf(node: Node, siblings: JsonObject): Check[] {
  const presets = node.check === undefined ? [] : [node.check];
  const by = node.checkBy;
  const selector = by === undefined ? undefined : siblings[by.field];
  const chosen = typeof selector === 'string' ? by?.cases.get(selector) : undefined;
  return chosen === undefined ? presets : [...presets, chosen];
}

function probeTree(tree: Tree, value: unknown, path: string, errors: FieldError[]): void {
  if (isObject(value)) {
    for (const [name, nested] of Object.entries(value)) {
      const node = tree.fields.get(name);
      if (node === undefined || nested === undefined) {
        continue;
      }
      const fieldPath = pushSegment(path, name);
      for (const preset of presetsOf(node, value)) {
        if (typeof nested === 'string') {
          probe(preset, nested, fieldPath, errors);
        } else if (Array.isArray(nested)) {
          nested.forEach((item, index) => {
            if (typeof item === 'string') {
              probe(preset, item, pushIndex(fieldPath, index), errors);
            }
          });
        }
      }
      probeTree(node.nested, nested, fieldPath, errors);
    }
    return;
  }
  if (Array.isArray(value) && tree.items !== undefined) {
    const nested = tree.items;
    value.forEach((item, index) => probeTree(nested, item, pushIndex(path, index), errors));
  }
}

function probe(preset: Check, text: string, path: string, errors: FieldError[]): void {
  if (check(preset, text)) {
    return;
  }
  errors.push({
    path,
    code: checkCode(preset),
    key: checkKey(preset),
    params: { value: text },
  });
}
