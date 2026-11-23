import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { CONTROLS, buildForm, mapControl, type PhxControl } from '@globalart/keel/forms';
import type { Control, Descriptor } from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');

const TABLE: Record<Control, PhxControl> = {
  input: 'phx-input',
  password: 'phx-password',
  select: 'phx-select',
  toggle: 'phx-toggle',
  switcher: 'phx-switcher',
  slider: 'phx-slider',
  date: 'phx-date-field',
  time: 'phx-time-field',
  file: 'phx-file-field',
  list: 'phx-list-field',
  code: 'phx-code-editor',
  tags: 'phx-tags-field',
  key_value: 'phx-key-value-field',
  transfer: 'phx-transfer-list',
};

function descriptorWith(property: Record<string, unknown>): Descriptor {
  return {
    keel: 1,
    name: 'demo.note.edit',
    module: 'demo',
    kind: 'form',
    mode: 'write',
    access: { level: 'user' },
    params: {
      type: 'object',
      properties: { value: property },
      $defs: { Level: { type: 'string', enum: ['user', 'admin'] } },
    },
    result: { type: 'object' },
    ui: { title: 'demo.note.edit.title' },
  };
}

function mappingOf(property: Record<string, unknown>) {
  const field = buildForm(descriptorWith(property)).field('value');
  if (field === undefined) {
    throw new Error('the synthetic descriptor has no field value');
  }
  return field.mapping;
}

describe('mapControl', () => {
  it('maps the fields of descriptor-dns-zone-edit.json onto phoenix components', () => {
    const form = buildForm(EDIT);
    expect(form.field('name')?.mapping).toEqual({
      component: 'phx-input',
      type: 'text',
      preset: 'domain',
    });
    const masterIps = form.field('master_ips');
    expect(masterIps?.mapping).toEqual({ component: 'phx-tags-field', preset: 'ip' });
    expect(masterIps?.schema['items']).toEqual({ type: 'string' });
    expect(form.field('tsig_key')?.mapping).toEqual({
      component: 'phx-password',
      generate: true,
      preset: null,
    });
    expect(form.field('account')?.mapping).toEqual({
      component: 'phx-select',
      choices: [],
      remote: 'keel.account.options',
      preset: null,
    });
    expect(form.field('ttl')?.mapping).toEqual({
      component: 'phx-number-field',
      min: 0,
      max: null,
      step: 1,
      unlimited: false,
      preset: null,
    });
    expect(form.field('kind')?.mapping).toEqual({
      component: 'phx-select',
      choices: ['master', 'slave'],
      remote: null,
      preset: null,
    });
    expect(form.field('id')).toBeUndefined();
    expect(form.fields.map((field) => field.name)).toEqual([
      'name',
      'kind',
      'ttl',
      'account',
      'master_ips',
      'tsig_key',
    ]);
  });

  it('the fourteen controls of the enum map onto the fourteen components', () => {
    expect(CONTROLS).toHaveLength(14);
    expect([...CONTROLS].sort()).toEqual(Object.keys(TABLE).sort());
    for (const control of CONTROLS) {
      const mapping = mappingOf({ type: 'string', 'x-ui': { control } });
      expect(mapping.component).toBe(TABLE[control]);
      expect(mapping.preset).toBeNull();
    }
  });

  it('a field without a control is mapped by its schema', () => {
    expect(mappingOf({ type: 'boolean' })).toEqual({ component: 'phx-toggle', preset: null });
    expect(mappingOf({ $ref: '#/$defs/Level' })).toEqual({
      component: 'phx-select',
      choices: ['user', 'admin'],
      remote: null,
      preset: null,
    });
    expect(mappingOf({ type: 'integer', minimum: 1, maximum: 10 })).toEqual({
      component: 'phx-number-field',
      min: 1,
      max: 10,
      step: 1,
      unlimited: false,
      preset: null,
    });
    expect(mappingOf({ type: ['number', 'null'] })).toEqual({
      component: 'phx-number-field',
      min: null,
      max: null,
      step: 1,
      unlimited: true,
      preset: null,
    });
    expect(mappingOf({ type: 'string', format: 'date' })).toEqual({
      component: 'phx-date-field',
      preset: null,
    });
    expect(mappingOf({ type: 'array', items: { type: 'string' } })).toEqual({
      component: 'phx-tags-field',
      preset: null,
    });
    expect(mappingOf({ type: 'string', format: 'email' })).toEqual({
      component: 'phx-input',
      type: 'email',
      preset: null,
    });
    expect(mappingOf({ type: 'array', items: { type: 'integer' } })).toEqual({
      component: 'phx-input',
      type: 'text',
      preset: null,
    });
  });

  it('a field marked secret is phx-password whatever its control says', () => {
    expect(
      mappingOf({ type: 'string', 'x-keel': { secret: true }, 'x-ui': { control: 'input' } }),
    ).toEqual({ component: 'phx-password', generate: false, preset: null });
  });

  it('a nullable number offers unlimited unless x-ui says a plain number, on a slider too', () => {
    const bounds = { type: ['integer', 'null'], minimum: 0, maximum: 65_535 };
    expect(mappingOf({ ...bounds, 'x-ui': { unlimited: false } })).toEqual({
      component: 'phx-number-field',
      min: 0,
      max: 65_535,
      step: 1,
      unlimited: false,
      preset: null,
    });
    expect(mappingOf(bounds)).toEqual({
      component: 'phx-number-field',
      min: 0,
      max: 65_535,
      step: 1,
      unlimited: true,
      preset: null,
    });
    expect(
      mappingOf({
        anyOf: [{ type: 'integer', minimum: 1, maximum: 64 }, { type: 'null' }],
        'x-ui': { control: 'slider', unlimited: false },
      }),
    ).toEqual({
      component: 'phx-slider',
      min: 1,
      max: 64,
      step: 1,
      unlimited: false,
      preset: null,
    });
  });

  it('a slider takes its bounds from the schema and unlimited from nullability', () => {
    expect(
      mappingOf({
        anyOf: [{ type: 'integer', minimum: 1, maximum: 64 }, { type: 'null' }],
        'x-ui': { control: 'slider' },
      }),
    ).toEqual({ component: 'phx-slider', min: 1, max: 64, step: 1, unlimited: true, preset: null });
    expect(mappingOf({ type: 'integer', 'x-ui': { control: 'slider' } })).toEqual({
      component: 'phx-slider',
      min: 0,
      max: 100,
      step: 1,
      unlimited: false,
      preset: null,
    });
  });

  it('a list field takes its columns from the properties of the items', () => {
    expect(
      mappingOf({
        type: 'array',
        items: {
          type: 'object',
          properties: { host: { type: 'string' }, port: { type: 'integer' } },
        },
        'x-ui': { control: 'list' },
      }),
    ).toEqual({ component: 'phx-list-field', columns: ['host', 'port'], preset: null });
  });

  it('a key-value field takes its key pattern from propertyNames', () => {
    expect(
      mappingOf({
        type: 'object',
        additionalProperties: { type: 'string' },
        propertyNames: { pattern: '^[A-Z_][A-Z0-9_]*$' },
        'x-ui': { control: 'key_value' },
      }),
    ).toEqual({ component: 'phx-key-value-field', keyPattern: '^[A-Z_][A-Z0-9_]*$', preset: null });
    expect(mappingOf({ type: 'object', 'x-ui': { control: 'key_value' } })).toEqual({
      component: 'phx-key-value-field',
      keyPattern: null,
      preset: null,
    });
  });

  it('a transfer list takes its choices from the enum of the items or its options function', () => {
    expect(
      mappingOf({
        type: 'array',
        items: { type: 'string', enum: ['dns', 'mail', 'web'] },
        'x-ui': { control: 'transfer' },
      }),
    ).toEqual({
      component: 'phx-transfer-list',
      choices: ['dns', 'mail', 'web'],
      remote: null,
      preset: null,
    });
    expect(
      mappingOf({
        type: 'array',
        items: { type: 'string' },
        'x-keel': { options: 'dns.zone.options' },
        'x-ui': { control: 'transfer' },
      }),
    ).toEqual({
      component: 'phx-transfer-list',
      choices: [],
      remote: 'dns.zone.options',
      preset: null,
    });
  });

  it('mapControl is the pure table behind the model', () => {
    expect(
      mapControl({
        name: 'name',
        schema: { type: 'string' },
        nullable: false,
        type: 'string',
        keel: { check: 'domain' },
        ui: {},
        defs: {},
      }),
    ).toEqual({ component: 'phx-input', type: 'text', preset: 'domain' });
  });
});
