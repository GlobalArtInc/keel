import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { FormControl } from '@angular/forms';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import {
  KeelFieldComponent,
  buildForm,
  fieldValidator,
  findingsOf,
  isEmptyValue,
  validateField,
  type FormField,
} from '@globalart/keel/forms';
import type { Descriptor, Envelope, FieldError } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const ENVELOPE_422 = load<Envelope>('envelope-422.json');

const MESSAGES = {
  keel: {
    check: { ip: 'Not an IP address: {value}' },
    schema: { minimum: 'Must be at least {limit}', required: 'Required' },
  },
};

function fixtureError(path: string): FieldError {
  if (ENVELOPE_422.ok) {
    throw new Error('envelope-422.json is not an error envelope');
  }
  const entry = (ENVELOPE_422.error.fields ?? []).find((field) => field.path === path);
  if (entry === undefined) {
    throw new Error(`envelope-422.json has no entry for ${path}`);
  }
  return entry;
}

function fieldOf(name: string, values: Record<string, unknown> = {}): FormField {
  const field = buildForm(EDIT, { values }).field(name);
  if (field === undefined) {
    throw new Error(`no field ${name}`);
  }
  return field;
}

async function render(field: FormField): Promise<ComponentFixture<KeelFieldComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelFieldComponent],
    providers: [
      providePhoenixIcons(),
      {
        provide: KeelClient,
        useValue: new KeelMockClient({ catalogue: { lang: 'en', messages: MESSAGES } }),
      },
    ],
  });
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelFieldComponent);
  fixture.componentRef.setInput('field', field);
  fixture.detectChanges();
  return fixture;
}

function errorText(fixture: ComponentFixture<KeelFieldComponent>): string | null {
  const element: HTMLElement = fixture.nativeElement;
  return element.querySelector('phx-field-error .text')?.textContent?.trim() ?? null;
}

describe('validators', () => {
  it('master_ips set to ["10.0.0.1", "10.0.0"] marks the control invalid with the FieldError of envelope-422.json', () => {
    const field = fieldOf('master_ips', { kind: 'slave' });
    field.control.setValue(['10.0.0.1', '10.0.0']);
    expect(field.control.invalid).toBe(true);
    const expected = fixtureError('master_ips[1]');
    expect(expected).toEqual({
      path: 'master_ips[1]',
      code: 'check.ip',
      key: 'keel.check.ip',
      params: { value: '10.0.0' },
    });
    expect(findingsOf(field.control)).toEqual([expected]);
  });

  it('phx-field-error shows t("keel.check.ip", params) for the client-side finding', async () => {
    const field = fieldOf('master_ips', { kind: 'slave' });
    const fixture = await render(field);
    expect(fixture.nativeElement.querySelector('phx-tags-field')).not.toBeNull();
    expect(errorText(fixture)).toBeNull();

    field.control.setValue(['10.0.0.1', '10.0.0']);
    field.control.markAsTouched();
    fixture.detectChanges();
    expect(errorText(fixture)).toBe('Not an IP address: 10.0.0');
    expect(
      fixture.nativeElement.querySelector('phx-tags-field input')?.getAttribute('aria-invalid'),
    ).toBe('true');

    field.control.setValue(['10.0.0.1', '10.0.0.2']);
    fixture.detectChanges();
    expect(errorText(fixture)).toBeNull();
  });

  it('a ttl below the schema minimum shows keel.schema.minimum with the bound', async () => {
    const field = fieldOf('ttl');
    field.control.setValue(-1);
    expect(findingsOf(field.control)).toEqual([
      { path: 'ttl', code: 'schema.minimum', key: 'keel.schema.minimum', params: { limit: 0 } },
    ]);

    const fixture = await render(field);
    expect(fixture.nativeElement.querySelector('phx-number-field')).not.toBeNull();
    field.control.markAsDirty();
    fixture.detectChanges();
    expect(errorText(fixture)).toBe('Must be at least 0');
  });

  it('a required field left empty yields schema.required and an optional one nothing', () => {
    const name = fieldOf('name');
    expect(findingsOf(name.control)).toEqual([
      {
        path: 'name',
        code: 'schema.required',
        key: 'keel.schema.required',
        params: { property: 'name' },
      },
    ]);
    name.control.setValue('a.org');
    expect(findingsOf(name.control)).toEqual([]);
    name.control.setValue('not a domain');
    expect(findingsOf(name.control)).toEqual([
      {
        path: 'name',
        code: 'check.domain',
        key: 'keel.check.domain',
        params: { value: 'not a domain' },
      },
    ]);

    const account = fieldOf('account');
    expect(account.control.valid).toBe(true);
    account.control.setValue('');
    expect(account.control.valid).toBe(true);
    account.control.setValue('not-an-id');
    expect(findingsOf(account.control).map((finding) => finding.code)).toEqual([
      'schema.minLength',
      'schema.pattern',
    ]);
  });

  it('validateField and fieldValidator agree on a rule', () => {
    const rule = {
      name: 'port',
      schema: { type: 'integer', maximum: 65535 },
      defs: {},
      required: () => true,
    };
    expect(validateField(rule, undefined)).toEqual([
      {
        path: 'port',
        code: 'schema.required',
        key: 'keel.schema.required',
        params: { property: 'port' },
      },
    ]);
    expect(validateField(rule, 70000)).toEqual([
      {
        path: 'port',
        code: 'schema.maximum',
        key: 'keel.schema.maximum',
        params: { limit: 65535 },
      },
    ]);
    expect(validateField(rule, 443)).toEqual([]);
    const control = new FormControl<unknown>(70000, { validators: fieldValidator(rule) });
    expect(control.errors).toEqual({ keel: validateField(rule, 70000) });
    control.setValue(443);
    expect(control.errors).toBeNull();
    expect(findingsOf(control)).toEqual([]);
    expect(findingsOf(null)).toEqual([]);
  });

  it('a check chosen by a sibling follows that sibling as it changes', () => {
    const record: Descriptor = {
      ...EDIT,
      name: 'dns.record.create',
      key: undefined,
      ui: undefined,
      params: {
        type: 'object',
        properties: {
          type: { type: 'string', enum: ['A', 'AAAA'] },
          value: {
            type: 'string',
            'x-keel': { checkBy: { field: 'type', cases: { A: 'ipv4', AAAA: 'ipv6' } } },
          },
        },
      },
    };
    const form = buildForm(record, { values: { type: 'A', value: '2001:db8::1' } });
    const value = form.field('value');
    if (value === undefined) {
      throw new Error('no field value');
    }
    expect(findingsOf(value.control).map((finding) => finding.code)).toEqual(['check.ipv4']);
    form.group.controls['type']?.setValue('AAAA');
    expect(findingsOf(value.control)).toEqual([]);
    value.control.setValue('192.0.2.1');
    expect(findingsOf(value.control).map((finding) => finding.code)).toEqual(['check.ipv6']);
    form.destroy();
  });

  it('isEmptyValue treats null, undefined, the empty string and an empty array as absent', () => {
    expect([null, undefined, '', []].map(isEmptyValue)).toEqual([true, true, true, true]);
    expect([0, false, ' ', [''], {}].map(isEmptyValue)).toEqual([
      false,
      false,
      false,
      false,
      false,
    ]);
  });
});
