import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import {
  derivedKeys,
  entityPrefix,
  format,
  lookup,
  sharedKey,
  sharedKeys,
  type Descriptor,
  type Messages,
} from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function descriptor(name: string): Descriptor {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as Descriptor;
}

function keysOf(name: string): string[] {
  return derivedKeys(descriptor(name)).map((derived) => derived.key);
}

describe('derivedKeys', () => {
  it('derives the column, status, action and value keys of descriptor-dns-zone-list.json', () => {
    const keys = keysOf('descriptor-dns-zone-list.json');
    expect(keys).toEqual(
      expect.arrayContaining([
        'dns.zone.column.name',
        'dns.zone.status.active',
        'dns.zone.action.suspend',
        'dns.zone.column.records',
      ]),
    );
    expect(keys).toEqual([
      'dns.zone.action.create',
      'dns.zone.action.edit',
      'dns.zone.action.records',
      'dns.zone.action.suspend',
      'dns.zone.action.delete',
      'dns.zone.action.export',
      'dns.zone.column.name',
      'dns.zone.column.records',
      'dns.zone.column.status',
      'dns.zone.status.active',
      'dns.zone.status.suspended',
      'dns.zone.column.dnssec',
      'dns.zone.column.account',
    ]);
  });

  it('derives the field, enum and page keys of descriptor-dns-zone-edit.json in page order', () => {
    const keys = derivedKeys(descriptor('descriptor-dns-zone-edit.json'));
    expect(keys.map((derived) => derived.key)).toEqual([
      'dns.zone.name',
      'dns.zone.kind',
      'dns.zone.kind.master',
      'dns.zone.kind.slave',
      'dns.zone.ttl',
      'dns.zone.account',
      'dns.zone.master_ips',
      'dns.zone.tsig_key',
      'dns.zone.page.main',
      'dns.zone.page.transfer',
    ]);
    expect(keys[0]).toEqual({ member: 'field name', key: 'dns.zone.name', shared: 'keel.name' });
    expect(keys[2]).toEqual({
      member: 'field kind value master',
      key: 'dns.zone.kind.master',
      shared: 'keel.kind.master',
    });
    expect(keys[8]).toEqual({
      member: 'page main',
      key: 'dns.zone.page.main',
      shared: 'keel.page.main',
    });
  });

  it('a form without pages derives every property but the key, and a screenless function none', () => {
    const edit = descriptor('descriptor-dns-zone-edit.json');
    const flat: Descriptor = { ...edit, ui: { title: 'dns.zone.edit.title' } };
    expect(derivedKeys(flat).map((derived) => derived.key)).toEqual([
      'dns.zone.name',
      'dns.zone.kind',
      'dns.zone.kind.master',
      'dns.zone.kind.slave',
      'dns.zone.master_ips',
      'dns.zone.ttl',
      'dns.zone.tsig_key',
      'dns.zone.account',
    ]);
    expect(derivedKeys({ ...edit, ui: null })).toEqual([]);
    expect(entityPrefix('dns.zone.edit')).toBe('dns.zone');
    expect(entityPrefix('bare')).toBe('bare');
  });

  it('a filter option and a status value of one column are derived once', () => {
    const twice: Descriptor = {
      ...descriptor('descriptor-dns-zone-list.json'),
      ui: {
        title: 'dns.zone.list.title',
        columns: [
          {
            key: 'status',
            type: 'status',
            map: { suspended: 'warning', active: 'success' },
            filter: { options: ['suspended', 'archived'] },
          },
        ],
      },
    };
    expect(derivedKeys(twice).map((derived) => derived.key)).toEqual([
      'dns.zone.column.status',
      'dns.zone.status.suspended',
      'dns.zone.status.archived',
      'dns.zone.status.active',
    ]);
  });
});

describe('lookup', () => {
  const catalogue: Messages = {
    dns: { zone: { list: { title: 'Zones' }, empty: 'No zones', column: { name: 'Zone' } } },
    'dns.zone.edit.title': 'Zone',
    keel: {
      column: { createdAt: 'Created' },
      name: 'Name',
      page: { main: 'Main' },
      title: 'Title',
    },
    menu: { dns: 'DNS' },
  };

  it('answers keel.column.createdAt for a missing dns.zone.column.createdAt', () => {
    expect(sharedKey('dns.zone.column.createdAt')).toBe('keel.column.createdAt');
    expect(lookup(catalogue, 'dns.zone.column.createdAt')).toBe('Created');
    expect(lookup(catalogue, 'dns.zone.column.name')).toBe('Zone');
    expect(lookup(catalogue, 'dns.zone.name')).toBe('Name');
    expect(lookup(catalogue, 'dns.zone.column.updatedAt')).toBeUndefined();
    expect(sharedKey('menu.dns')).toBe('keel.dns');
    expect(lookup(catalogue, 'menu.other')).toBeUndefined();
  });

  it('a function without an entity segment inherits the tail after its module, as the kernel derives it', () => {
    expect(sharedKeys('settings.page.main')).toEqual(['keel.main', 'keel.page.main']);
    expect(lookup(catalogue, 'settings.page.main')).toBe('Main');
    expect(lookup(catalogue, 'note.title')).toBe('Title');
    expect(sharedKeys('keel.page.main')).toEqual(['keel.main']);
    expect(sharedKeys('keel.account.kind')).toEqual(['keel.kind']);
    expect(sharedKeys('title')).toEqual([]);
    expect(lookup(catalogue, 'keel.page.other')).toBeUndefined();
  });

  it('walks nested objects and flat entries alike, as the kernel does', () => {
    expect(lookup(catalogue, 'dns.zone.list.title')).toBe('Zones');
    expect(lookup(catalogue, 'dns.zone.empty')).toBe('No zones');
    expect(lookup(catalogue, 'dns.zone.edit.title')).toBe('Zone');
    expect(lookup(catalogue, 'menu.dns')).toBe('DNS');
    expect(lookup(catalogue, 'dns.zone')).toBeUndefined();
    expect(lookup(catalogue, 'dns.zone.list')).toBeUndefined();
    expect(lookup(catalogue, 'dns.zone.get.title')).toBeUndefined();
    expect(lookup(catalogue, 'constructor')).toBeUndefined();
    expect(lookup(catalogue, 'dns.constructor.x')).toBeUndefined();
  });
});

describe('format', () => {
  it('substitutes the single placeholder form and leaves 100% alone', () => {
    expect(format('The note {title} is locked', { title: 'x' })).toBe('The note x is locked');
    expect(format('100% sure', { title: 'x' })).toBe('100% sure');
    expect(format('50 % of {total}', { total: 3 })).toBe('50 % of 3');
    expect(format('{a} and {b_2} and {_c}', { a: true, b_2: 'B', _c: null })).toBe(
      'true and B and null',
    );
  });

  it('leaves a placeholder without a parameter and every other brace form untouched', () => {
    expect(format('Hello {name}')).toBe('Hello {name}');
    expect(format('Hello {name}', { name: undefined })).toBe('Hello {name}');
    expect(format('Hello {{name}}', { name: 'x' })).toBe('Hello {x}');
    expect(format('Hello {0} { name } {}', { '0': 'x', name: 'y' })).toBe('Hello {0} { name } {}');
    expect(format('Hello {name', { name: 'x' })).toBe('Hello {name');
  });
});
