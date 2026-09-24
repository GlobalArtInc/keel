import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import {
  ExprError,
  pushIndex,
  pushSegment,
  validate,
  type Descriptor,
  type Envelope,
  type FieldError,
  type Schema,
} from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load(name: string): unknown {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8'));
}

function zoneEditParams(): Schema {
  return (load('descriptor-dns-zone-edit.json') as Descriptor).params;
}

function summary(errors: FieldError[]): [string, string][] {
  return errors.map((error) => [error.path, error.code ?? '']);
}

const RECORDS: Schema = {
  type: 'object',
  properties: {
    name: { type: 'string', 'x-keel': { check: 'domain' } },
    records: {
      type: 'array',
      items: { $ref: '#/$defs/Record' },
    },
    primary: { anyOf: [{ $ref: '#/$defs/Record' }, { type: 'null' }] },
    nested: { anyOf: [{ $ref: '#/$defs/Nested' }, { type: 'null' }] },
  },
  required: ['name'],
  $defs: {
    Record: {
      type: 'object',
      properties: {
        ip: { type: 'string', 'x-keel': { check: 'ip' } },
        ports: { type: 'array', items: { type: 'string' }, 'x-keel': { check: 'port' } },
      },
      required: ['ip', 'ports'],
    },
    Nested: {
      type: 'object',
      properties: { depth: { type: 'integer', minimum: 0, maximum: 255 } },
      required: ['depth'],
    },
  },
};

describe('validate against the params of descriptor-dns-zone-edit.json', () => {
  it('a bad master ip yields the one entry of envelope-422.json', () => {
    const errors = validate(zoneEditParams(), {
      name: 'a.org',
      kind: 'slave',
      master_ips: ['10.0.0.1', '10.0.0'],
      ttl: 300,
    });
    const envelope = load('envelope-422.json') as Envelope;
    const wire = envelope.ok ? [] : (envelope.error.fields ?? []);
    expect(errors).toEqual([wire[0]]);
    expect(errors).toEqual([
      {
        path: 'master_ips[1]',
        code: 'check.ip',
        key: 'keel.check.ip',
        params: { value: '10.0.0' },
      },
    ]);
  });

  it('a missing name yields schema.required on name', () => {
    const errors = validate(zoneEditParams(), { kind: 'slave', master_ips: [], ttl: 300 });
    expect(errors).toEqual([
      {
        path: 'name',
        code: 'schema.required',
        key: 'keel.schema.required',
        params: { property: 'name' },
      },
    ]);
  });

  it('a ttl below the minimum yields schema.minimum with the bound in params', () => {
    const errors = validate(zoneEditParams(), {
      name: 'a.org',
      kind: 'master',
      master_ips: [],
      ttl: -1,
    });
    expect(errors).toEqual([
      { path: 'ttl', code: 'schema.minimum', key: 'keel.schema.minimum', params: { limit: 0 } },
    ]);
  });

  it('a valid zone yields no error and an unknown kind fails the enum of its $defs', () => {
    expect(
      validate(zoneEditParams(), {
        id: '01J9Q3W6B1R8N5X2K4T7M0V9Z3',
        name: 'a.org',
        kind: 'master',
        master_ips: [],
        ttl: 300,
        tsig_key: null,
        account: null,
      }),
    ).toEqual([]);
    expect(
      validate(zoneEditParams(), { name: 'a.org', kind: 'forward', master_ips: [], ttl: 300 }),
    ).toEqual([
      { path: 'kind', code: 'schema.enum', key: 'keel.schema.enum', params: { value: 'forward' } },
    ]);
  });

  it('schema errors carry json paths and schema codes in the order of the kernel', () => {
    expect(
      summary(
        validate(zoneEditParams(), {
          name: 7,
          kind: 'master',
          master_ips: ['1.1.1.1', 2],
          ttl: 1.5,
        }),
      ),
    ).toEqual([
      ['name', 'schema.type'],
      ['master_ips[1]', 'schema.type'],
      ['ttl', 'schema.type'],
    ]);
    const errors = validate(zoneEditParams(), {
      name: 'a.org',
      kind: 'master',
      master_ips: [],
      ttl: 300,
      id: 'nope',
    });
    expect(summary(errors)).toEqual([
      ['id', 'schema.minLength'],
      ['id', 'schema.pattern'],
    ]);
    expect(errors[0].params).toEqual({ limit: 26 });
    expect(errors[1].params).toBeUndefined();
  });

  it('a nullable branch unfolds to the errors of its other branch and a wrong value to schema.anyOf', () => {
    expect(summary(validate(RECORDS, { name: 'a.org', nested: 4 }))).toEqual([
      ['nested', 'schema.type'],
    ]);
    expect(summary(validate(RECORDS, { name: 'a.org', nested: { depth: 'x' } }))).toEqual([
      ['nested.depth', 'schema.type'],
    ]);
    expect(summary(validate(RECORDS, { name: 'a.org', nested: { depth: 300 } }))).toEqual([
      ['nested.depth', 'schema.maximum'],
    ]);
    const both: Schema = {
      type: 'object',
      properties: { value: { anyOf: [{ type: 'string' }, { type: 'number' }] } },
    };
    expect(validate(both, { value: true })).toEqual([
      { path: 'value', code: 'schema.anyOf', key: 'keel.schema.anyOf' },
    ]);
  });
});

describe('validate applies check presets', () => {
  it('to strings and to every string item of an array', () => {
    const errors = validate(zoneEditParams(), {
      name: 'not a domain',
      kind: 'slave',
      master_ips: ['10.0.0.1', '10.0.0', '::1'],
      ttl: 1,
    });
    expect(errors).toEqual([
      {
        path: 'name',
        code: 'check.domain',
        key: 'keel.check.domain',
        params: { value: 'not a domain' },
      },
      {
        path: 'master_ips[1]',
        code: 'check.ip',
        key: 'keel.check.ip',
        params: { value: '10.0.0' },
      },
    ]);
  });

  it('below the top level with the full json path, through $defs, items and nullable branches', () => {
    const errors = validate(RECORDS, {
      name: 'a.org',
      records: [
        { ip: '10.0.0.1', ports: ['53'] },
        { ip: '10.0.0', ports: ['53', '70000'] },
      ],
      primary: { ip: '::x', ports: [] },
    });
    expect(summary(errors)).toEqual([
      ['records[1].ip', 'check.ip'],
      ['records[1].ports[1]', 'check.port'],
      ['primary.ip', 'check.ip'],
    ]);
    expect(
      validate(RECORDS, {
        name: 'a.org',
        records: [{ ip: '10.0.0.1', ports: ['53'] }],
        primary: null,
      }),
    ).toEqual([]);
  });

  it('chosen by a sibling value through checkBy, at any depth', () => {
    const entry = {
      type: 'object',
      properties: {
        kind: { type: 'string' },
        value: {
          type: 'string',
          'x-keel': { checkBy: { field: 'kind', cases: { v4: 'ipv4', text: 'quoted' } } },
        },
        entries: { type: 'array', items: { $ref: '#/$defs/Entry' } },
      },
    };
    const params = { ...entry, $defs: { Entry: entry } };
    expect(validate(params, { kind: 'v4', value: '192.0.2.1' })).toEqual([]);
    expect(validate(params, { kind: 'other', value: 'anything' })).toEqual([]);
    expect(
      summary(
        validate(params, {
          kind: 'v4',
          value: '2001:db8::1',
          entries: [
            { kind: 'text', value: '"ok"' },
            { kind: 'text', value: 'bare' },
          ],
        }),
      ),
    ).toEqual([
      ['value', 'check.ipv4'],
      ['entries[1].value', 'check.quoted'],
    ]);
  });

  it('after the schema errors, and never to a non-string value', () => {
    const errors = validate(zoneEditParams(), {
      name: 'not a domain',
      kind: 'slave',
      master_ips: [7],
      ttl: -1,
    });
    expect(summary(errors)).toEqual([
      ['master_ips[0]', 'schema.type'],
      ['ttl', 'schema.minimum'],
      ['name', 'check.domain'],
    ]);
  });
});

describe('validate covers the subset the design names', () => {
  const schema: Record<string, unknown> = {
    type: 'object',
    properties: {
      login: { type: 'string', minLength: 2, maxLength: 4 },
      code: { type: 'string', pattern: '^[a-z]+$' },
      day: { type: 'string', format: 'date' },
      at: { type: 'string', format: 'date-time' },
      mail: { type: 'string', format: 'email' },
      other: { type: 'string', format: 'uuid' },
      count: { type: ['integer', 'null'], maximum: 10 },
      flag: { type: 'boolean' },
      broken: { type: 'string', pattern: '(' },
    },
  };

  it('minLength and maxLength count code points', () => {
    expect(summary(validate(schema, { login: 'a' }))).toEqual([['login', 'schema.minLength']]);
    expect(validate(schema, { login: 'abcde' })).toEqual([
      {
        path: 'login',
        code: 'schema.maxLength',
        key: 'keel.schema.maxLength',
        params: { limit: 4 },
      },
    ]);
    expect(validate(schema, { login: '\u{1F600}\u{1F600}\u{1F600}\u{1F600}' })).toEqual([]);
  });

  it('pattern searches unanchored and an invalid pattern throws', () => {
    expect(validate(schema, { code: 'abc' })).toEqual([]);
    expect(validate(schema, { code: 'ab1' })).toEqual([
      { path: 'code', code: 'schema.pattern', key: 'keel.schema.pattern' },
    ]);
    expect(() => validate(schema, { broken: 'x' })).toThrow(ExprError);
  });

  it('date, date-time and email formats are asserted and other formats are ignored', () => {
    expect(validate(schema, { day: '2024-02-29' })).toEqual([]);
    expect(summary(validate(schema, { day: '2023-02-29' }))).toEqual([['day', 'schema.format']]);
    expect(summary(validate(schema, { day: '2024-13-01' }))).toEqual([['day', 'schema.format']]);
    expect(summary(validate(schema, { day: '2024-1-01' }))).toEqual([['day', 'schema.format']]);
    expect(validate(schema, { at: '2024-02-29T23:59:60Z' })).toEqual([]);
    expect(validate(schema, { at: '2024-02-29t12:00:00.5+02:00' })).toEqual([]);
    expect(validate(schema, { at: '2024-02-29T01:59:60+02:00' })).toEqual([]);
    expect(summary(validate(schema, { at: '2024-02-29T12:00:60Z' }))).toEqual([
      ['at', 'schema.format'],
    ]);
    expect(summary(validate(schema, { at: '2024-02-29 12:00:00Z' }))).toEqual([
      ['at', 'schema.format'],
    ]);
    expect(summary(validate(schema, { at: '2024-02-29T12:00:00' }))).toEqual([
      ['at', 'schema.format'],
    ]);
    expect(summary(validate(schema, { at: '2024-02-29T12:00:00.Z' }))).toEqual([
      ['at', 'schema.format'],
    ]);
    expect(summary(validate(schema, { at: '2024-02-29T12:00:00+2:00' }))).toEqual([
      ['at', 'schema.format'],
    ]);
    expect(validate(schema, { mail: 'alice@a.org' })).toEqual([]);
    expect(summary(validate(schema, { mail: 'alice@org' }))).toEqual([['mail', 'schema.format']]);
    expect(validate(schema, { other: 'anything' })).toEqual([]);
  });

  it('a type list accepts any of its members and a required member holding undefined is absent', () => {
    expect(validate(schema, { count: null })).toEqual([]);
    expect(validate(schema, { count: 5 })).toEqual([]);
    expect(summary(validate(schema, { count: 11 }))).toEqual([['count', 'schema.maximum']]);
    expect(summary(validate(schema, { count: 1.5 }))).toEqual([['count', 'schema.type']]);
    expect(summary(validate(schema, { flag: 'yes' }))).toEqual([['flag', 'schema.type']]);
    expect(validate(schema, { login: undefined })).toEqual([]);
    expect(summary(validate({ ...schema, required: ['flag'] }, { flag: undefined }))).toEqual([
      ['flag', 'schema.required'],
    ]);
  });

  it('a boolean schema accepts everything or nothing', () => {
    expect(validate(true, { anything: 1 })).toEqual([]);
    expect(summary(validate(false, {}))).toEqual([['', 'schema.schema']]);
  });
});

describe('paths', () => {
  it('segments join with dots and brackets', () => {
    expect(pushSegment(pushIndex(pushSegment('', 'records'), 1), 'ip')).toBe('records[1].ip');
  });
});
