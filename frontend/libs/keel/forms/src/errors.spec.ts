import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import {
  KeelFormComponent,
  applyCallError,
  applyFieldErrors,
  buildForm,
  clearFieldErrors,
  findingsOf,
  rootOf,
  type KeelForm,
} from '@globalart/keel/forms';
import type { CallError, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const ENVELOPE_422 = load<Envelope>('envelope-422.json');
const ENVELOPE_403 = load<Envelope>('envelope-403.json');

const MESSAGES = {
  dns: {
    zone: {
      edit: { title: 'Zone' },
      name: 'Zone name',
      kind: 'Kind',
      ttl: 'TTL',
      account: 'Account',
      master_ips: 'Master servers',
      tsig_key: 'TSIG key',
      page: { transfer: 'Transfer' },
      error: { exists: 'The zone {name} exists already' },
    },
  },
  keel: {
    page: { main: 'Main' },
    check: { ip: 'Not an IP address: {value}' },
    error: {
      forbidden_field: 'The field is above your level',
      conflict: 'Somebody changed it first',
    },
  },
};

function errorOf(envelope: Envelope): CallError {
  if (envelope.ok) {
    throw new Error('the fixture is not an error envelope');
  }
  return envelope.error;
}

async function render(form: KeelForm): Promise<ComponentFixture<KeelFormComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelFormComponent],
    providers: [
      providePhoenixIcons(),
      {
        provide: KeelClient,
        useValue: new KeelMockClient({ catalogue: { lang: 'en', messages: MESSAGES } }),
      },
    ],
  });
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelFormComponent);
  fixture.componentRef.setInput('form', form);
  fixture.detectChanges();
  return fixture;
}

function fieldError(
  fixture: ComponentFixture<KeelFormComponent>,
  component: string,
): string | null {
  const element: HTMLElement = fixture.nativeElement;
  return element.querySelector(`${component} phx-field-error .text`)?.textContent?.trim() ?? null;
}

function bannerEntries(fixture: ComponentFixture<KeelFormComponent>): string[] {
  const element: HTMLElement = fixture.nativeElement;
  return [...element.querySelectorAll('phx-banner .banner__entry')].map((entry) =>
    [...entry.children].map((part) => part.textContent?.trim() ?? '').join(' '),
  );
}

describe('errors', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('rootOf strips indices and nested segments from a path', () => {
    expect(rootOf('master_ips[1]')).toBe('master_ips');
    expect(rootOf('records[1].ip')).toBe('records');
    expect(rootOf('primary.ip')).toBe('primary');
    expect(rootOf('name')).toBe('name');
    expect(rootOf('')).toBe('');
  });

  it('envelope-422.json lands check.ip on the control master_ips[1] and dns.zone.error.exists on name with the translated messages', async () => {
    const form = buildForm(EDIT, {
      values: { name: 'a.org', kind: 'slave', ttl: 300, master_ips: ['10.0.0.1', '10.0.0.2'] },
    });
    const fixture = await render(form);
    expect(fieldError(fixture, 'phx-tags-field')).toBeNull();

    const error = errorOf(ENVELOPE_422);
    expect(applyCallError(form, error)).toEqual([]);
    fixture.detectChanges();
    expect(findingsOf(form.group.controls['master_ips'])).toEqual([error.fields?.[0]]);
    expect(findingsOf(form.group.controls['name'])).toEqual([error.fields?.[1]]);
    expect(form.group.controls['master_ips'].touched).toBe(true);
    expect(fieldError(fixture, 'phx-tags-field')).toBe('Not an IP address: 10.0.0');
    expect(fieldError(fixture, 'phx-input')).toBe('The zone a.org exists already');
    expect(fixture.nativeElement.querySelector('phx-banner')).toBeNull();

    form.group.controls['name'].setValue('b.org');
    fixture.detectChanges();
    expect(findingsOf(form.group.controls['name'])).toEqual([]);
    expect(fieldError(fixture, 'phx-input')).toBeNull();
    expect(fieldError(fixture, 'phx-tags-field')).toBe('Not an IP address: 10.0.0');
  });

  it('envelope-403.json lands forbidden_field on account', async () => {
    const form = buildForm(EDIT, { values: { name: 'a.org', kind: 'master', ttl: 300 } });
    const fixture = await render(form);
    expect(applyCallError(form, errorOf(ENVELOPE_403))).toEqual([]);
    fixture.detectChanges();
    expect(findingsOf(form.group.controls['account'])).toEqual([
      { path: 'account', code: 'forbidden_field', key: 'keel.error.forbidden_field' },
    ]);
    expect(fieldError(fixture, 'phx-select')).toBe('The field is above your level');
  });

  it('an unknown path shows on the banner', async () => {
    const form = buildForm(EDIT, { values: { name: 'a.org', kind: 'master', ttl: 300 } });
    const fixture = await render(form);
    const stray = [
      { path: 'records[0].ip', code: 'check.ip', key: 'keel.check.ip', params: { value: '::x' } },
      { path: 'master_ips[0]', code: 'check.ip', key: 'keel.check.ip', params: { value: '1' } },
    ];
    expect(applyFieldErrors(form, stray)).toEqual(stray);
    expect(form.unmapped()).toEqual(stray);
    fixture.detectChanges();
    expect(bannerEntries(fixture)).toEqual([
      'records[0].ip Not an IP address: ::x',
      'master_ips[0] Not an IP address: 1',
    ]);
    expect(fixture.nativeElement.querySelector('phx-banner')?.getAttribute('role')).toBe('alert');

    clearFieldErrors(form);
    fixture.detectChanges();
    expect(fixture.nativeElement.querySelector('phx-banner')).toBeNull();
  });

  it('an error without fields shows its own message on the banner', async () => {
    const form = buildForm(EDIT, { values: { name: 'a.org', kind: 'master', ttl: 300 } });
    const fixture = await render(form);
    applyCallError(form, { code: 'conflict', key: 'keel.error.conflict', requestId: 'req-9' });
    fixture.detectChanges();
    expect(bannerEntries(fixture)).toEqual(['Somebody changed it first']);
    expect(fixture.nativeElement.querySelector('.banner__path')).toBeNull();
  });
});
