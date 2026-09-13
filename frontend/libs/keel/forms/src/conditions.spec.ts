import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { buildForm, findingsOf, type FormField, type FormPage } from '@globalart/keel/forms';
import type { Descriptor } from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const ZONE_ID = '01J8A0000000000000000000Z1';

function fieldOf(name: string, form = buildForm(EDIT)): FormField {
  const field = form.field(name);
  if (field === undefined) {
    throw new Error(`no field ${name}`);
  }
  return field;
}

function pageOf(id: string, form = buildForm(EDIT)): FormPage {
  const page = form.page(id);
  if (page === undefined) {
    throw new Error(`no page ${id}`);
  }
  return page;
}

describe('conditions', () => {
  it('the page transfer is shown only while kind is slave', () => {
    const form = buildForm(EDIT);
    const transfer = pageOf('transfer', form);
    expect(form.mode).toBe('create');
    expect(transfer.visible()).toBe(false);
    expect(form.visiblePages().map((page) => page.id)).toEqual(['main']);
    expect(form.layout()).toBe('section');

    form.group.controls['kind'].setValue('slave');
    expect(transfer.visible()).toBe(true);
    expect(form.visiblePages().map((page) => page.id)).toEqual(['main', 'transfer']);
    expect(form.layout()).toBe('tabs');
    expect(fieldOf('master_ips', form).visible()).toBe(true);
    expect(fieldOf('master_ips', form).control.enabled).toBe(true);

    form.group.controls['kind'].setValue('master');
    expect(transfer.visible()).toBe(false);
    expect(form.layout()).toBe('section');
    expect(fieldOf('master_ips', form).control.disabled).toBe(true);
  });

  it('master_ips is required while kind is slave and not otherwise', () => {
    const form = buildForm(EDIT, { values: { name: 'a.org', ttl: 300 } });
    const masterIps = fieldOf('master_ips', form);
    expect(masterIps.required()).toBe(false);

    form.group.controls['kind'].setValue('slave');
    expect(masterIps.required()).toBe(true);
    expect(masterIps.control.invalid).toBe(true);
    expect(form.group.invalid).toBe(true);
    expect(findingsOf(masterIps.control)).toEqual([
      {
        path: 'master_ips',
        code: 'schema.required',
        key: 'keel.schema.required',
        params: { property: 'master_ips' },
      },
    ]);

    masterIps.control.setValue(['10.0.0.1']);
    expect(masterIps.control.valid).toBe(true);

    form.group.controls['kind'].setValue('master');
    expect(masterIps.required()).toBe(false);
    expect(masterIps.control.disabled).toBe(true);
    expect(form.group.valid).toBe(true);
  });

  it('name is read-only in edit mode and editable in create', () => {
    const edit = buildForm(EDIT, { params: { id: ZONE_ID }, values: { name: 'a.org' } });
    expect(edit.mode).toBe('edit');
    expect(edit.context().mode).toBe('edit');
    const name = fieldOf('name', edit);
    expect(name.readonly()).toBe(true);
    expect(name.visible()).toBe(true);
    expect(name.control.disabled).toBe(true);
    expect(edit.group.getRawValue()['name']).toBe('a.org');
    expect(edit.group.getRawValue()['id']).toBe(ZONE_ID);
    expect(edit.values()['name']).toBe('a.org');

    const create = buildForm(EDIT);
    expect(fieldOf('name', create).readonly()).toBe(false);
    expect(fieldOf('name', create).control.enabled).toBe(true);
    expect(create.group.getRawValue()['id']).toBeNull();

    const explicit = buildForm(EDIT, { mode: 'edit' });
    expect(fieldOf('name', explicit).readonly()).toBe(true);
  });

  it('the model of a create with kind master marks master_ips and tsig_key hidden', () => {
    const form = buildForm(EDIT, { values: { kind: 'master' } });
    expect(form.mode).toBe('create');
    expect(fieldOf('master_ips', form).visible()).toBe(false);
    expect(fieldOf('tsig_key', form).visible()).toBe(false);
    expect(fieldOf('master_ips', form).control.disabled).toBe(true);
    expect(fieldOf('tsig_key', form).control.disabled).toBe(true);
    expect(fieldOf('name', form).visible()).toBe(true);
    expect(Object.keys(form.group.value)).toEqual(['id', 'name', 'kind', 'ttl', 'account']);
    expect(Object.keys(form.group.getRawValue())).toEqual([
      'id',
      'name',
      'kind',
      'master_ips',
      'ttl',
      'tsig_key',
      'account',
    ]);
  });

  it('conditions see $principal.level and $screen.params', () => {
    const descriptor: Descriptor = {
      keel: 1,
      name: 'dns.record.edit',
      module: 'dns',
      kind: 'form',
      mode: 'write',
      access: { level: 'user' },
      params: {
        type: 'object',
        properties: { ttl: { type: 'integer' }, note: { type: 'string' } },
      },
      result: { type: 'object' },
      ui: {
        title: 'dns.record.edit.title',
        pages: [{ id: 'main', fields: ['ttl', 'note'] }],
        fields: {
          ttl: { readonly_if: { eq: ['$principal.level', 'user'] } },
          note: { show_if: { eq: ['$screen.params.zone_id', 'z1'] } },
        },
      },
    };
    const user = buildForm(descriptor, { principal: { level: 'user' }, params: { zone_id: 'z1' } });
    expect(fieldOf('ttl', user).readonly()).toBe(true);
    expect(fieldOf('note', user).visible()).toBe(true);

    const admin = buildForm(descriptor, {
      principal: { level: 'admin' },
      params: { zone_id: 'z2' },
    });
    expect(fieldOf('ttl', admin).readonly()).toBe(false);
    expect(fieldOf('note', admin).visible()).toBe(false);

    const anonymous = buildForm(descriptor);
    expect(fieldOf('ttl', anonymous).readonly()).toBe(false);
    expect(fieldOf('note', anonymous).visible()).toBe(false);
  });

  it('destroy stops the model from following the group', () => {
    const form = buildForm(EDIT);
    form.destroy();
    form.group.controls['kind'].setValue('slave');
    expect(pageOf('transfer', form).visible()).toBe(false);
  });
});
