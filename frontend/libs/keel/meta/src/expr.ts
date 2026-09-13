import type { Context, Expr } from './generated';
import { equal, isObject, type JsonObject } from './json';

export class ExprError extends Error {
  constructor(
    message: string,
    readonly pattern?: string,
  ) {
    super(message);
    this.name = 'ExprError';
  }
}

export function evaluate(expr: Expr, context: Context): boolean {
  if ('eq' in expr) {
    return equal(resolve(context, expr.eq[0]), expr.eq[1]);
  }
  if ('ne' in expr) {
    return !equal(resolve(context, expr.ne[0]), expr.ne[1]);
  }
  if ('gt' in expr) {
    return compare(resolve(context, expr.gt[0]), expr.gt[1]) === 1;
  }
  if ('lt' in expr) {
    return compare(resolve(context, expr.lt[0]), expr.lt[1]) === -1;
  }
  if ('in' in expr) {
    const value = resolve(context, expr.in[0]);
    return expr.in[1].some((item) => equal(value, item));
  }
  if ('matches' in expr) {
    const regex = compilePattern(expr.matches[1]);
    const value = resolve(context, expr.matches[0]);
    return typeof value === 'string' && regex.test(value);
  }
  if ('truthy' in expr) {
    return truthy(resolve(context, expr.truthy));
  }
  if ('empty' in expr) {
    return empty(resolve(context, expr.empty));
  }
  if ('and' in expr) {
    return expr.and.every((item) => evaluate(item, context));
  }
  if ('or' in expr) {
    return expr.or.some((item) => evaluate(item, context));
  }
  if ('not' in expr) {
    return !evaluate(expr.not, context);
  }
  throw new ExprError(`unknown operator in ${JSON.stringify(expr)}`);
}

export function compilePattern(pattern: string): RegExp {
  try {
    return new RegExp(pattern, 'u');
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    throw new ExprError(`invalid pattern ${JSON.stringify(pattern)}: ${reason}`, pattern);
  }
}

export function resolve(context: Context, variable: string): unknown {
  if (variable.startsWith('$')) {
    return resolveSystem(context, variable.slice(1));
  }
  return walk(context.fields ?? {}, variable.split('.'));
}

function resolveSystem(context: Context, path: string): unknown {
  const [head, ...rest] = path.split('.');
  switch (head) {
    case 'mode':
      return rest.length === 0 && typeof context.mode === 'string' ? context.mode : null;
    case 'principal':
      return rest.length === 1 && rest[0] === 'level' && context.principal
        ? context.principal.level
        : null;
    case 'screen':
      return rest[0] === 'params' && context.screen
        ? walk(context.screen.params ?? {}, rest.slice(1))
        : null;
    default:
      return null;
  }
}

function walk(root: JsonObject, segments: string[]): unknown {
  let current: unknown = root;
  for (const segment of segments) {
    if (!isObject(current) || !Object.hasOwn(current, segment)) {
      return null;
    }
    current = current[segment];
  }
  return current ?? null;
}

function compare(left: unknown, right: unknown): number | undefined {
  if (typeof left === 'number' && typeof right === 'number') {
    if (Number.isNaN(left) || Number.isNaN(right)) {
      return undefined;
    }
    return left < right ? -1 : left > right ? 1 : 0;
  }
  if (typeof left === 'string' && typeof right === 'string') {
    return compareStrings(left, right);
  }
  return undefined;
}

function compareStrings(left: string, right: string): number {
  let i = 0;
  let j = 0;
  while (i < left.length && j < right.length) {
    const a = left.codePointAt(i) ?? 0;
    const b = right.codePointAt(j) ?? 0;
    if (a !== b) {
      return a < b ? -1 : 1;
    }
    i += a > 0xffff ? 2 : 1;
    j += b > 0xffff ? 2 : 1;
  }
  const leftRemains = i < left.length;
  const rightRemains = j < right.length;
  if (leftRemains === rightRemains) {
    return 0;
  }
  return leftRemains ? 1 : -1;
}

function truthy(value: unknown): boolean {
  if (value === null || value === undefined) {
    return false;
  }
  if (typeof value === 'boolean') {
    return value;
  }
  if (typeof value === 'number') {
    return value !== 0;
  }
  if (typeof value === 'string') {
    return value.length > 0;
  }
  if (Array.isArray(value)) {
    return value.length > 0;
  }
  if (isObject(value)) {
    return Object.keys(value).length > 0;
  }
  return false;
}

function empty(value: unknown): boolean {
  if (value === null || value === undefined) {
    return true;
  }
  if (typeof value === 'string') {
    return value.length === 0;
  }
  if (Array.isArray(value)) {
    return value.length === 0;
  }
  if (isObject(value)) {
    return Object.keys(value).length === 0;
  }
  return false;
}
