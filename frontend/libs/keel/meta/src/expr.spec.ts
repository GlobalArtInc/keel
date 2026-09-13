import { readFileSync } from 'node:fs';
import { join, resolve as resolvePath } from 'node:path';
import { ExprError, evaluate, resolve, type Context, type Expr } from '@globalart/keel/meta';

const FIXTURES = resolvePath(__dirname, '../../../../../contracts/fixtures');

interface ExprCase {
  name: string;
  source: string;
  invalid?: boolean;
  ast?: Expr;
  context?: Context;
  expect?: boolean;
}

function cases(): ExprCase[] {
  return JSON.parse(readFileSync(join(FIXTURES, 'expr-cases.json'), 'utf8')) as ExprCase[];
}

function fields(values: Record<string, unknown>): Context {
  return { fields: values };
}

function caught(run: () => unknown): unknown {
  try {
    run();
  } catch (error) {
    return error;
  }
  return undefined;
}

describe('evaluate', () => {
  it('every valid case of expr-cases.json evaluates to its expectation', () => {
    const found = cases();
    expect(found.length).toBeGreaterThanOrEqual(60);
    const valid = found.filter((entry) => !entry.invalid);
    expect(valid.length).toBeGreaterThan(0);
    for (const entry of valid) {
      expect({ name: entry.name, ast: entry.ast, expect: entry.expect }).toEqual({
        name: entry.name,
        ast: expect.anything(),
        expect: expect.any(Boolean),
      });
      const value = evaluate(entry.ast as Expr, entry.context ?? {});
      expect({ name: entry.name, value }).toEqual({ name: entry.name, value: entry.expect });
    }
  });

  it('an invalid case of expr-cases.json carries no ast, since the client never parses source', () => {
    for (const entry of cases().filter((found) => found.invalid)) {
      expect(entry.ast).toBeUndefined();
      expect(entry.expect).toBeUndefined();
    }
  });

  it('a matches with an invalid pattern throws', () => {
    const broken: Expr = { matches: ['name', '('] };
    expect(() => evaluate(broken, fields({ name: 'x' }))).toThrow(ExprError);
    expect(() => evaluate(broken, fields({}))).toThrow('invalid pattern "("');
    const thrown = caught(() => evaluate(broken, fields({})));
    expect(thrown).toBeInstanceOf(ExprError);
    expect((thrown as ExprError).pattern).toBe('(');
  });

  it('and and or short-circuit before a broken pattern', () => {
    const broken: Expr = { matches: ['name', '('] };
    const context = fields({ flag: false, name: 'x' });
    expect(evaluate({ and: [{ truthy: 'flag' }, broken] }, context)).toBe(false);
    expect(evaluate({ or: [{ not: { truthy: 'flag' } }, broken] }, context)).toBe(true);
  });

  it('matches applies a regular expression to strings only', () => {
    const ast: Expr = { matches: ['name', '^[a-z]+\\.org$'] };
    expect(evaluate(ast, fields({ name: 'a.org' }))).toBe(true);
    expect(evaluate(ast, fields({ name: 'A.org' }))).toBe(false);
    expect(evaluate(ast, fields({ name: 7 }))).toBe(false);
  });

  it('numbers compare by value and mismatched types never compare greater or less', () => {
    expect(evaluate({ eq: ['ttl', 300] }, fields({ ttl: 300.0 }))).toBe(true);
    expect(evaluate({ gt: ['ttl', 299.5] }, fields({ ttl: 300 }))).toBe(true);
    expect(evaluate({ lt: ['ttl', 300] }, fields({ ttl: 300 }))).toBe(false);
    expect(evaluate({ in: ['ttl', [100, 300]] }, fields({ ttl: 300 }))).toBe(true);
    expect(evaluate({ gt: ['ttl', 300] }, fields({ ttl: '600' }))).toBe(false);
    expect(evaluate({ lt: ['ttl', 300] }, fields({ ttl: '600' }))).toBe(false);
    expect(evaluate({ ne: ['ttl', 300] }, fields({ ttl: '600' }))).toBe(true);
  });

  it('strings compare by code point as bytes of utf-8 do', () => {
    expect(evaluate({ gt: ['name', '\uffff'] }, fields({ name: '\u{10000}' }))).toBe(true);
    expect(evaluate({ lt: ['name', '\uffff'] }, fields({ name: '\u{10000}' }))).toBe(false);
    expect(evaluate({ gt: ['name', 'ab'] }, fields({ name: 'abc' }))).toBe(true);
    expect(evaluate({ lt: ['name', 'abc'] }, fields({ name: 'ab' }))).toBe(true);
  });

  it('truthy and empty follow the value kind', () => {
    const table: [unknown, boolean, boolean][] = [
      [null, false, true],
      [undefined, false, true],
      [false, false, false],
      [true, true, false],
      [0, false, false],
      [2, true, false],
      ['', false, true],
      ['x', true, false],
      [[], false, true],
      [[1], true, false],
      [{}, false, true],
      [{ a: 1 }, true, false],
    ];
    for (const [value, isTruthy, isEmpty] of table) {
      const context = fields({ v: value });
      expect({ value, truthy: evaluate({ truthy: 'v' }, context) }).toEqual({
        value,
        truthy: isTruthy,
      });
      expect({ value, empty: evaluate({ empty: 'v' }, context) }).toEqual({
        value,
        empty: isEmpty,
      });
    }
  });

  it('an unknown operator throws rather than answering false', () => {
    const unknown = { gte: ['ttl', 300] } as unknown as Expr;
    expect(() => evaluate(unknown, fields({ ttl: 600 }))).toThrow(ExprError);
  });
});

describe('resolve', () => {
  const context: Context = {
    fields: { ttl: 300, owner: { login: 'alice' }, tags: ['a'] },
    mode: 'edit',
    principal: { level: 'reseller' },
    screen: { params: { zone_id: 'z1' } },
  };

  it('resolves fields and system variables and answers null for anything else', () => {
    expect(resolve(context, 'ttl')).toBe(300);
    expect(resolve(context, 'owner.login')).toBe('alice');
    expect(resolve(context, 'owner.missing')).toBeNull();
    expect(resolve(context, 'tags.0')).toBeNull();
    expect(resolve(context, 'missing')).toBeNull();
    expect(resolve(context, 'constructor')).toBeNull();
    expect(resolve(context, '$mode')).toBe('edit');
    expect(resolve(context, '$mode.x')).toBeNull();
    expect(resolve(context, '$principal.level')).toBe('reseller');
    expect(resolve(context, '$principal.login')).toBeNull();
    expect(resolve(context, '$principal')).toBeNull();
    expect(resolve(context, '$screen.params.zone_id')).toBe('z1');
    expect(resolve(context, '$screen.params')).toEqual({ zone_id: 'z1' });
    expect(resolve(context, '$screen')).toBeNull();
    expect(resolve(context, '$unknown')).toBeNull();
    expect(resolve({}, '$principal.level')).toBeNull();
    expect(resolve({}, 'ttl')).toBeNull();
  });

  it('a field holding undefined resolves to null', () => {
    expect(resolve({ fields: { ttl: undefined } }, 'ttl')).toBeNull();
    expect(evaluate({ eq: ['ttl', null] }, { fields: { ttl: undefined } })).toBe(true);
  });
});
