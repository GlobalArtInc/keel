import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import Ajv2020, { type ValidateFunction } from 'ajv/dist/2020';
import addFormats from 'ajv-formats';
import type {
  Account,
  ActionRole,
  Boot,
  BootMenuItem,
  Check,
  ColumnType,
  Control,
  Envelope,
  ErrorCode,
  Expr,
  Kind,
  Level,
  Page,
} from '@globalart/keel/meta';

const CONTRACTS = resolve(__dirname, '../../../../../contracts');
const FIXTURES = join(CONTRACTS, 'fixtures');
const SCHEMA = join(CONTRACTS, 'schema');

const KNOWN = [
  'boot-none.json',
  'boot.json',
  'check-cases.json',
  'descriptor-dns-zone-edit.json',
  'descriptor-dns-zone-list.json',
  'envelope-200.json',
  'envelope-403.json',
  'envelope-422.json',
  'error-codes.json',
  'event.json',
  'expr-cases.json',
  'list-query.json',
  'page.json',
];

const INTEGER_FORMATS: Record<string, [number, number]> = {
  int32: [-(2 ** 31), 2 ** 31 - 1],
  uint16: [0, 2 ** 16 - 1],
  uint32: [0, 2 ** 32 - 1],
  uint64: [0, Number.MAX_SAFE_INTEGER],
};

function load(name: string): unknown {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8'));
}

function validators(): (type: string) => ValidateFunction {
  const ajv = new Ajv2020({ allowUnionTypes: true, strict: true });
  addFormats(ajv);
  for (const [name, [min, max]] of Object.entries(INTEGER_FORMATS)) {
    ajv.addFormat(name, {
      type: 'number',
      validate: (value: number) => Number.isInteger(value) && value >= min && value <= max,
    });
  }
  ajv.addFormat('double', { type: 'number', validate: (value: number) => Number.isFinite(value) });
  ajv.addSchema(JSON.parse(readFileSync(join(SCHEMA, 'keel.json'), 'utf8')));
  const compiled = new Map<string, ValidateFunction>();
  return (type) => {
    let validate = compiled.get(type);
    if (validate === undefined) {
      validate = ajv.compile(JSON.parse(readFileSync(join(SCHEMA, `${type}.json`), 'utf8')));
      compiled.set(type, validate);
    }
    return validate;
  };
}

const validator = validators();

function expectValid(type: string, value: unknown, label: string): void {
  const validate = validator(type);
  const valid = validate(value);
  expect({ label, errors: validate.errors ?? [] }).toEqual({ label, errors: [] });
  expect(valid).toBe(true);
}

interface ExprCase {
  name: string;
  invalid?: boolean;
  ast?: unknown;
}

interface CheckCase {
  check: string;
}

interface CodeRow {
  code: string;
}

describe('contracts/fixtures', () => {
  it('every fixture file has a test', () => {
    const present = readdirSync(FIXTURES)
      .filter((name) => statSync(join(FIXTURES, name)).isFile())
      .sort();
    expect(present).toEqual([...KNOWN].sort());
  });

  it('the two descriptors validate against Descriptor', () => {
    for (const name of ['descriptor-dns-zone-list.json', 'descriptor-dns-zone-edit.json']) {
      expectValid('Descriptor', load(name), name);
    }
  });

  it('a descriptor with an unknown kind is refused', () => {
    const descriptor = { ...(load('descriptor-dns-zone-list.json') as object), kind: 'table' };
    const validate = validator('Descriptor');
    expect(validate(descriptor)).toBe(false);
    expect(validate.errors?.some((error) => error.instancePath === '/kind')).toBe(true);
  });

  it('the three envelopes validate against Envelope', () => {
    for (const name of ['envelope-200.json', 'envelope-403.json', 'envelope-422.json']) {
      expectValid('Envelope', load(name), name);
    }
  });

  it('list-query.json validates against ListQuery', () => {
    expectValid('ListQuery', load('list-query.json'), 'list-query.json');
  });

  it('page.json validates against Page', () => {
    expectValid('Page', load('page.json'), 'page.json');
  });

  it('boot.json validates against Boot', () => {
    expectValid('Boot', load('boot.json'), 'boot.json');
  });

  it('boot-none.json validates against Boot with no product, no view and the system group alone', () => {
    const boot = load('boot-none.json') as Boot;
    expectValid('Boot', boot, 'boot-none.json');
    expect(boot.products).toEqual([]);
    expect(boot.views).toEqual([]);
    expect(boot.brand.name).toBe('keel.brand');
    expect(boot.menu.map((group) => [group.id, group.product])).toEqual([['system', null]]);
    expect(boot.menu[0].items.every((item) => item.kind !== undefined)).toBe(true);
  });

  it('a boot with a view and its menu item without a kind validates against Boot', () => {
    const item: BootMenuItem = {
      path: 'hosting/files',
      label: 'hosting.file_manager.title',
      func: 'hosting.file-manager',
    };
    const boot = load('boot.json') as Boot;
    boot.views = ['hosting.file-manager'];
    boot.menu[1].items.push(item);
    expectValid('Boot', boot, 'boot.json with a view');
    boot.menu[1].items.push({ path: 'hosting/terminal', label: 'x' } as BootMenuItem);
    expect(validator('Boot')(boot)).toBe(false);
  });

  it('event.json validates against Event', () => {
    expectValid('Event', load('event.json'), 'event.json');
  });

  it('the ast of every valid case of expr-cases.json validates against Expr', () => {
    const cases = load('expr-cases.json') as ExprCase[];
    expect(cases.length).toBeGreaterThanOrEqual(60);
    for (const found of cases) {
      if (found.invalid) {
        expect(found.ast).toBeUndefined();
        continue;
      }
      expectValid('Expr', found.ast, found.name);
    }
    const ast: Expr = { and: [{ eq: ['kind', 'slave'] }, { not: { truthy: 'dnssec' } }] };
    expectValid('Expr', ast, 'a typed ast');
  });

  it('every code of error-codes.json validates against ErrorCode', () => {
    const rows = load('error-codes.json') as CodeRow[];
    expect(rows).toHaveLength(13);
    for (const row of rows) {
      expectValid('ErrorCode', row.code, row.code);
    }
    expect(validator('ErrorCode')('teapot')).toBe(false);
  });

  it('the check member of every check-cases.json case validates against Check', () => {
    const cases = load('check-cases.json') as CheckCase[];
    const presets = new Set(cases.map((found) => found.check));
    expect(presets.size).toBe(12);
    for (const preset of presets) {
      expectValid('Check', preset, preset);
    }
    expect(validator('Check')('phone')).toBe(false);
  });
});

type IsString<T> = T extends string ? (string extends T ? true : false) : false;
type IsLiteralUnion<T> = string extends T ? false : T extends string ? true : false;

describe('closed vocabulary', () => {
  it('Kind, Control, ColumnType, ActionRole, Check, ErrorCode and Level are string-literal unions', () => {
    const unions: [
      IsLiteralUnion<Kind>,
      IsLiteralUnion<Control>,
      IsLiteralUnion<ColumnType>,
      IsLiteralUnion<ActionRole>,
      IsLiteralUnion<Check>,
      IsLiteralUnion<ErrorCode>,
      IsLiteralUnion<Level>,
    ] = [true, true, true, true, true, true, true];
    const kind: Kind = 'list';
    expect(unions.every(Boolean)).toBe(true);
    expect(validator('Kind')(kind)).toBe(true);
    expect(validator('Kind')('table')).toBe(false);
  });
});

describe('generic aliases', () => {
  type Rows = Extract<Envelope<Page<Account>>, { ok: true }>['data']['data'];

  it('Envelope<Page<Account>> types data.data[0].id as a string', () => {
    const idIsString: IsString<Rows[number]['id']> = true;
    const envelope: Envelope<Page<Account>> = {
      ok: true,
      data: {
        data: [
          {
            id: '01J8A0000000000000000000A1',
            kind: 'reseller',
            path: '/01J8A0000000000000000000A1/',
            state: 'active',
          },
        ],
        total: 1,
      },
      meta: { requestId: 'req-1' },
    };
    const id: string = envelope.ok ? envelope.data.data[0].id : '';
    expect(idIsString).toBe(true);
    expect(id).toBe('01J8A0000000000000000000A1');
    expectValid('Envelope', envelope, 'a typed envelope');
  });

  it('an error envelope narrows on ok', () => {
    const envelope: Envelope<Page<Account>> = {
      ok: false,
      error: { code: 'not_found', key: 'keel.error.not_found' },
    };
    expect(envelope.ok ? 'data' : envelope.error.code).toBe('not_found');
  });
});
