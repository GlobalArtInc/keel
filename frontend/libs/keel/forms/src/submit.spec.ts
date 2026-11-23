import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import {
  KeelCallError,
  KeelClient,
  KeelI18n,
  PARAMS_PART,
  multipartOf,
} from '@globalart/keel/client';
import {
  KeelFormComponent,
  bodyOf,
  buildForm,
  filesOf,
  findingsOf,
  loadForm,
  loadValues,
  newIdempotencyKey,
  type KeelForm,
  type Saved,
} from '@globalart/keel/forms';
import type { Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';
import { throwError } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const ENVELOPE_200 = load<Envelope>('envelope-200.json');
const ENVELOPE_422 = load<Envelope>('envelope-422.json');
const ZONE_ID = '01J9Q3W6B1R8N5X2K4T7M0V9Z3';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const TOKEN_CREATE: Descriptor = {
  keel: 1,
  name: 'auth.token.create',
  module: 'auth',
  kind: 'form',
  mode: 'write',
  access: { level: 'user' },
  params: {
    type: 'object',
    properties: { name: { type: 'string' } },
    required: ['name'],
  },
  result: {
    type: 'object',
    properties: {
      id: { $ref: '#/$defs/Id' },
      token: { type: 'string', 'x-keel': { once: true } },
    },
    required: ['id', 'token'],
    $defs: { Id: { type: 'string' } },
  },
  ui: { title: 'auth.token.create.title', pages: [{ id: 'main', fields: ['name'] }] },
};

const BRAND_LOGO: Descriptor = {
  keel: 1,
  name: 'settings.brand.logo',
  module: 'settings',
  kind: 'upload',
  mode: 'write',
  access: { level: 'admin' },
  key: 'host',
  params: {
    type: 'object',
    properties: {
      host: { type: 'string' },
      logo: {
        type: 'string',
        'x-keel': { file: { maxBytes: 65536, types: ['image/svg+xml'] } },
        'x-ui': { control: 'file' },
      },
    },
    required: ['host', 'logo'],
  },
  result: { type: 'object' },
  ui: {
    title: 'settings.brand.logo.title',
    pages: [{ id: 'main', fields: ['host', 'logo'] }],
    fields: { host: { readonly_if: { eq: ['$mode', 'edit'] } } },
  },
};

const MESSAGES = {
  auth: { token: { create: { title: 'New API token' }, name: 'Name', token: 'Token' } },
  dns: { zone: { edit: { title: 'Zone' } } },
  keel: {
    action: { save: 'Save' },
    once: { title: 'Shown once' },
    page: { main: 'Main' },
  },
};

async function render(
  form: KeelForm,
  client: KeelMockClient,
): Promise<ComponentFixture<KeelFormComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelFormComponent],
    providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelFormComponent);
  fixture.componentRef.setInput('form', form);
  fixture.detectChanges();
  return fixture;
}

describe('submit', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('a submit posts one Idempotency-Key and a 422 answer maps and leaves the key for the retry', async () => {
    const client = new KeelMockClient({
      answers: { 'dns.zone.edit': [ENVELOPE_422, ENVELOPE_200, ENVELOPE_200] },
    });
    const form = buildForm(EDIT, { client, values: { name: 'a.org', kind: 'master', ttl: 300 } });

    const refused = await form.submit();
    expect(refused?.ok).toBe(false);
    expect(client.calls).toHaveLength(1);
    const key = client.calls[0].options.idempotencyKey;
    expect(key).toMatch(UUID);
    expect(findingsOf(form.group.controls['name'])).toHaveLength(1);
    expect(form.pending()).toBe(false);

    expect(await form.submit()).toBeNull();
    expect(client.calls).toHaveLength(1);

    form.group.controls['name'].setValue('b.org');
    const saved = await form.submit();
    expect(saved?.ok).toBe(true);
    expect(client.calls).toHaveLength(2);
    expect(client.calls[1].options.idempotencyKey).toBe(key);

    await form.submit();
    expect(client.calls).toHaveLength(3);
    const fresh = client.calls[2].options.idempotencyKey;
    expect(fresh).toMatch(UUID);
    expect(fresh).not.toBe(key);
    form.destroy();
  });

  it('a create with kind master posts a body with no master_ips and no tsig_key', async () => {
    const client = new KeelMockClient({ answers: { 'dns.zone.edit': ENVELOPE_200 } });
    const form = buildForm(EDIT, { client, values: { name: 'a.org', kind: 'master', ttl: 300 } });
    expect(form.mode).toBe('create');
    expect(bodyOf(form)).toEqual({ name: 'a.org', kind: 'master', ttl: 300, account: null });
    await form.submit();
    expect(client.calls[0].params).toEqual({
      name: 'a.org',
      kind: 'master',
      ttl: 300,
      account: null,
    });

    form.group.controls['kind'].setValue('slave');
    form.group.controls['master_ips'].setValue(['10.0.0.1']);
    expect(bodyOf(form)).toEqual({
      name: 'a.org',
      kind: 'slave',
      ttl: 300,
      account: null,
      master_ips: ['10.0.0.1'],
      tsig_key: null,
    });
    form.destroy();
  });

  it('an upload descriptor submits multipart with a params part and one part per file field and the idempotency header', async () => {
    const client = new KeelMockClient({ answers: { 'settings.brand.logo': ENVELOPE_200 } });
    const form = buildForm(BRAND_LOGO, { client, params: { host: 'panel.example' } });
    expect(form.mode).toBe('edit');
    const host = form.field('host');
    expect(host?.control.value).toBe('panel.example');
    expect(host?.readonly()).toBe(true);
    expect(buildForm(BRAND_LOGO, { client }).field('host')?.readonly()).toBe(false);
    expect(buildForm(EDIT, { client }).field('id')).toBeUndefined();
    const svg = new File(['<svg xmlns="http://www.w3.org/2000/svg"/>'], 'logo.svg', {
      type: 'image/svg+xml',
    });
    form.group.controls['logo'].setValue([svg]);
    expect(bodyOf(form)).toEqual({ host: 'panel.example' });
    expect(filesOf(form)).toEqual({ logo: [svg] });

    const saved = await form.submit();
    expect(saved?.ok).toBe(true);
    expect(client.calls.map((call) => call.func)).toEqual(['settings.brand.logo']);
    expect(client.uploads).toHaveLength(1);
    expect(client.uploads[0].params).toEqual({ host: 'panel.example' });
    expect(client.uploads[0].files).toEqual({ logo: [svg] });
    expect(client.uploads[0].options.idempotencyKey).toMatch(UUID);

    const body = multipartOf({ host: 'panel.example' }, { logo: [svg] });
    const parts = [...body.entries()];
    expect(parts.map(([name]) => name)).toEqual([PARAMS_PART, 'logo']);
    const params = parts[0][1];
    expect(params).toBeInstanceOf(Blob);
    expect(await (params as Blob).text()).toBe('{"host":"panel.example"}');
    expect((params as Blob).type).toBe('application/json');
    const part = parts[1][1];
    expect(part).toBeInstanceOf(File);
    expect((part as File).name).toBe('logo.svg');
    expect((part as File).type).toBe('image/svg+xml');
    form.destroy();
  });

  it('an edit posts the key parameter and the read-only name it keeps', async () => {
    const client = new KeelMockClient({
      answers: {
        'dns.zone.edit': ENVELOPE_200,
        'keel.account.options': {
          ok: true,
          data: { data: [{ value: '01J8A0000000000000000000A1', label: 'Root' }], more: false },
          meta: { requestId: 'req-account' },
        },
      },
    });
    const form = buildForm(EDIT, {
      client,
      params: { id: ZONE_ID },
      values: { name: 'a.org', kind: 'master', ttl: 300, account: '01J8A0000000000000000000A1' },
    });
    expect(form.mode).toBe('edit');
    expect(form.group.controls['name'].disabled).toBe(true);
    await form.submit();
    expect(client.callsOf('dns.zone.edit')[0]?.params).toEqual({
      id: ZONE_ID,
      name: 'a.org',
      kind: 'master',
      ttl: 300,
      account: '01J8A0000000000000000000A1',
    });
    form.destroy();
  });

  it('a submit is refused while a required field is empty and marks the fields touched', async () => {
    const client = new KeelMockClient({ answers: { 'dns.zone.edit': ENVELOPE_200 } });
    const form = buildForm(EDIT, { client, values: { kind: 'master' } });
    expect(await form.submit()).toBeNull();
    expect(client.calls).toHaveLength(0);
    expect(form.group.controls['name'].touched).toBe(true);
    expect(findingsOf(form.group.controls['name']).map((finding) => finding.code)).toEqual([
      'schema.required',
    ]);
    form.destroy();
  });

  it('a transport failure lands on the banner as unavailable and keeps the key', async () => {
    const client = new KeelMockClient({ answers: { 'dns.zone.edit': ENVELOPE_200 } });
    const failing = jest
      .spyOn(client, 'call')
      .mockReturnValueOnce(throwError(() => new Error('connection refused')));
    const form = buildForm(EDIT, { client, values: { name: 'a.org', kind: 'master', ttl: 300 } });
    const failed = await form.submit();
    expect(failed?.ok).toBe(false);
    expect(form.unmapped()).toEqual([
      {
        path: '',
        code: 'unavailable',
        key: 'keel.error.unavailable',
        params: { reason: 'connection refused' },
      },
    ]);
    const key = failing.mock.calls[0][2]?.idempotencyKey;
    expect(key).toMatch(UUID);

    const saved = await form.submit();
    expect(saved?.ok).toBe(true);
    expect(form.unmapped()).toEqual([]);
    expect(client.calls[0].options.idempotencyKey).toBe(key);
    form.destroy();
  });

  it('a form built without a client refuses to submit', async () => {
    const form = buildForm(EDIT, { values: { name: 'a.org', kind: 'master', ttl: 300 } });
    await expect(form.submit()).rejects.toThrow('without a client');
    form.destroy();
  });

  it('newIdempotencyKey is a fresh UUID each time', () => {
    const first = newIdempotencyKey();
    expect(first).toMatch(UUID);
    expect(newIdempotencyKey()).not.toBe(first);
  });

  it('an edit loads the values of ui.read through dns.zone.get with the key parameter', async () => {
    const zone = { id: ZONE_ID, name: 'a.org', kind: 'slave', ttl: 60, master_ips: ['10.0.0.1'] };
    const client = new KeelMockClient({
      answers: { 'dns.zone.get': { ok: true, data: zone, meta: { requestId: 'req-get' } } },
    });
    expect(await loadValues(EDIT, { id: ZONE_ID }, client)).toEqual(zone);
    expect(client.calls).toEqual([{ func: 'dns.zone.get', params: { id: ZONE_ID }, options: {} }]);

    expect(await loadValues(EDIT, {}, client)).toEqual({});
    expect(client.calls).toHaveLength(1);

    const form = await loadForm(EDIT, { client, params: { id: ZONE_ID } });
    expect(form.mode).toBe('edit');
    expect(form.group.getRawValue()).toEqual({ ...zone, tsig_key: null, account: null });
    expect(form.page('transfer')?.visible()).toBe(true);
    expect(form.client).toBe(client);
    form.destroy();
  });

  it('a failed read rejects with the call error', async () => {
    const client = new KeelMockClient({
      answers: {
        'dns.zone.get': { ok: false, error: { code: 'not_found', key: 'keel.error.not_found' } },
      },
    });
    await expect(loadValues(EDIT, { id: ZONE_ID }, client)).rejects.toBeInstanceOf(KeelCallError);
  });

  it('the save button submits, emits submitted, and a result with a field marked once renders one phx-clipboard row with its value', async () => {
    const client = new KeelMockClient({
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'auth.token.create': {
          ok: true,
          data: { id: '01J8A0000000000000000000T1', token: 'kat_0123' },
          meta: { requestId: 'req-token' },
        },
      },
    });
    const form = buildForm(TOKEN_CREATE, { client, values: { name: 'ci' } });
    const fixture = await render(form, client);
    const element: HTMLElement = fixture.nativeElement;
    const submitted: Saved[] = [];
    fixture.componentInstance.submitted.subscribe((saved) => submitted.push(saved));
    expect(element.querySelector('keel-once')).toBeNull();

    const save = element.querySelector<HTMLButtonElement>('phx-form-footer button.save');
    expect(save?.textContent?.trim()).toBe('Save');
    save?.click();
    await fixture.whenStable();
    await Promise.resolve();
    fixture.detectChanges();

    expect(client.calls.map((call) => call.func)).toEqual(['auth.token.create']);
    expect(submitted).toHaveLength(1);
    expect(submitted[0].data).toEqual({ id: '01J8A0000000000000000000T1', token: 'kat_0123' });
    const clipboards = element.querySelectorAll('keel-once phx-clipboard');
    expect(clipboards).toHaveLength(1);
    expect(element.querySelector('keel-once phx-banner .title')?.textContent).toBe('Shown once');
    expect(element.querySelector('keel-once phx-prop .label-text')?.textContent?.trim()).toBe(
      'Token',
    );
    expect(element.querySelector('keel-once phx-prop .value-text')?.textContent?.trim()).toBe(
      'kat_0123',
    );
    fixture.destroy();
  });

  it('a form without a client renders no save button', async () => {
    const client = new KeelMockClient({ catalogue: { lang: 'en', messages: MESSAGES } });
    const fixture = await render(buildForm(TOKEN_CREATE), client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('phx-form-footer')).toBeNull();
    fixture.destroy();
  });
});
