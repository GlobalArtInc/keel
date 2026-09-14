import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { buildForm } from '@globalart/keel/forms';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelNavigation, KeelScreenComponent } from '@globalart/keel/renderer';
import {
  SETTINGS_GET,
  SETTINGS_SET,
  isSettingRow,
  settingKeyOf,
  provideKeelShellViews,
  valueDescriptor,
  valueSchema,
  valuesOf,
  type SettingRow,
} from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const SNAPSHOTS = resolve(__dirname, '../../../../../crates/system/tests/snapshots');
const MIN_LENGTH = 'auth.password.min_length';

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function snapshot(func: string, level: string): Descriptor {
  const text = readFileSync(join(SNAPSHOTS, `${func}@${level}.snap`), 'utf8');
  const body = text.slice(text.indexOf('\n---\n', text.indexOf('---\n') + 4) + 5);
  return JSON.parse(body) as Descriptor;
}

const SET = snapshot(SETTINGS_SET, 'admin');
const BOOT = load<Boot>('boot-none.json');

const MESSAGES = {
  settings: {
    set: { title: 'Setting' },
    key: 'Key',
    value: 'Value',
    error: { level: 'The setting {key} is at level {level}, above your level {caller}' },
  },
  keel: {
    page: { main: 'Main' },
    schema: { minimum: 'The value is too small' },
    action: { save: 'Save', retry: 'Retry', done: '{action} completed' },
    value: { true: 'Yes', false: 'No' },
    error: {
      not_found: 'Not found',
      forbidden: 'You are not allowed to do this',
      request: '{message} (request {id})',
    },
    shell: { settings: { title: 'Setting {key}', value: 'Value', default: 'Default' } },
  },
};

function row(overrides: Partial<SettingRow> = {}): SettingRow {
  return {
    key: MIN_LENGTH,
    module: 'auth',
    schema: { type: 'integer', format: 'uint32', minimum: 0, title: 'uint32' },
    default: 12,
    level: 'admin',
    value: 12,
    scope: null,
    ...overrides,
  };
}

function ok(data: unknown, requestId = 'req-setting'): Envelope {
  return { ok: true, data, meta: { requestId } };
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

async function render(
  answers: Record<string, Envelope | Envelope[]>,
  params: Record<string, unknown> = { key: MIN_LENGTH },
): Promise<ComponentFixture<KeelScreenComponent>> {
  const client = new KeelMockClient({
    boot: BOOT,
    descriptors: [SET],
    catalogue: { lang: 'en', messages: MESSAGES },
    answers,
  });
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [
      providePhoenixIcons(),
      provideKeelShellViews(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', SETTINGS_SET);
  fixture.componentRef.setInput('params', params);
  fixture.detectChanges();
  await settle(fixture);
  await settle(fixture);
  return fixture;
}

function overlay(): HTMLElement {
  const container = document.querySelector<HTMLElement>('.cdk-overlay-container');
  if (container === null) {
    throw new Error('no overlay container');
  }
  return container;
}

describe('the settings value form of the shell', () => {
  afterEach(() => {
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('the form of settings.set for auth.password.min_length renders phx-number-field from the row schema and posts { key, value }', async () => {
    const fixture = await render({
      [SETTINGS_GET]: ok(row()),
      [SETTINGS_SET]: ok(row({ value: 16, scope: BOOT.principal?.account }), 'req-saved'),
    });
    const element: HTMLElement = fixture.nativeElement;
    const client = TestBed.inject(KeelClient) as KeelMockClient;
    const back = jest.spyOn(TestBed.inject(KeelNavigation), 'back').mockImplementation(() => {});
    expect(element.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('keel-form-screen')).toBeNull();
    const view = element.querySelector('keel-settings-value');
    expect(view?.getAttribute('data-setting')).toBe(MIN_LENGTH);
    expect(view?.getAttribute('data-phase')).toBe('ready');
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      [SETTINGS_GET, { key: MIN_LENGTH }],
    ]);
    expect(element.querySelector('phx-form-sheet h1')?.textContent).toBe(`Setting ${MIN_LENGTH}`);
    const key = element.querySelector<HTMLInputElement>('phx-input input');
    expect(key?.value).toBe(MIN_LENGTH);
    expect(key?.disabled).toBe(true);
    const number = element.querySelector<HTMLInputElement>('phx-number-field input');
    expect(number?.value).toBe('12');
    expect(element.querySelector('[data-default]')?.textContent?.trim()).toBe('12');
    const labels = [...element.querySelectorAll('phx-form-row .label__text')].map(
      (label) => label.textContent,
    );
    expect(labels).toEqual(['Key', 'Value', 'Default']);

    if (number === null) {
      throw new Error('no number field');
    }
    number.value = '16';
    number.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    element.querySelector<HTMLButtonElement>('button.save')?.click();
    await settle(fixture);
    expect(client.callsOf(SETTINGS_SET).map((call) => call.params)).toEqual([
      { key: MIN_LENGTH, value: 16 },
    ]);
    expect(client.callsOf(SETTINGS_SET)[0].options.idempotencyKey).toMatch(/^[0-9a-f-]{36}$/);
    expect(overlay().querySelector('phx-toast')?.textContent).toContain(
      `Setting ${MIN_LENGTH} completed`,
    );
    expect(back).toHaveBeenCalledTimes(1);
  });

  it('a refused save lands on the value field and a refused read offers a retry', async () => {
    const fixture = await render({
      [SETTINGS_GET]: [
        {
          ok: false,
          error: { code: 'forbidden', key: 'keel.error.forbidden', requestId: 'req-denied' },
        },
        ok(row()),
      ],
      [SETTINGS_SET]: {
        ok: false,
        error: {
          code: 'validation',
          key: 'keel.error.validation',
          fields: [{ path: 'value', code: 'minimum', key: 'keel.schema.minimum' }],
        },
      },
    });
    const element: HTMLElement = fixture.nativeElement;
    const view = element.querySelector('keel-settings-value');
    expect(view?.getAttribute('data-phase')).toBe('failed');
    expect(element.querySelector('phx-error .text')?.textContent?.trim()).toBe(
      'You are not allowed to do this (request req-denied)',
    );
    element.querySelector<HTMLButtonElement>('phx-error button')?.click();
    await settle(fixture);
    expect(view?.getAttribute('data-phase')).toBe('ready');

    element.querySelector<HTMLButtonElement>('button.save')?.click();
    await settle(fixture);
    expect(element.querySelector('phx-number-field phx-field-error')?.textContent?.trim()).toBe(
      'The value is too small',
    );
    expect(element.querySelector('keel-findings')).toBeNull();
    expect(view?.getAttribute('data-phase')).toBe('ready');
  });

  it('a missing key shows the not-found page without reading a row', async () => {
    const fixture = await render({ [SETTINGS_GET]: ok(row()) }, {});
    const element: HTMLElement = fixture.nativeElement;
    const client = TestBed.inject(KeelClient) as KeelMockClient;
    expect(element.querySelector('keel-settings-value')?.getAttribute('data-phase')).toBe('failed');
    expect(element.querySelector('phx-not-found')).not.toBeNull();
    expect(client.calls).toHaveLength(0);
    expect(settingKeyOf({})).toBeNull();
    expect(settingKeyOf({ key: '' })).toBeNull();
    expect(settingKeyOf({ key: MIN_LENGTH })).toBe(MIN_LENGTH);
  });

  it('valueDescriptor puts the row schema on the value property and keeps the definitions', () => {
    const boolean = valueDescriptor(SET, row({ schema: { type: 'boolean' }, default: false }));
    expect(buildForm(boolean).field('value')?.mapping.component).toBe('phx-toggle');
    const text = valueDescriptor(
      SET,
      row({ schema: { $schema: 'https://json-schema.org/draft/2020-12/schema', type: 'string' } }),
    );
    expect(buildForm(text).field('value')?.mapping.component).toBe('phx-input');
    expect(valueSchema(true)).toEqual({});
    expect(valueSchema({ $schema: 'x', $defs: { A: {} }, type: 'string' })).toEqual({
      type: 'string',
    });
    const referenced = valueDescriptor(
      SET,
      row({
        schema: { $ref: '#/$defs/Tone', $defs: { Tone: { type: 'string', enum: ['a', 'b'] } } },
      }),
    );
    const params = referenced.params as Record<string, unknown>;
    expect(params['$defs']).toEqual({ Tone: { type: 'string', enum: ['a', 'b'] } });
    expect(buildForm(referenced).field('value')?.mapping).toEqual({
      component: 'phx-select',
      choices: ['a', 'b'],
      remote: null,
      preset: null,
    });
    expect(SET.params).not.toHaveProperty('$defs');
    expect(valuesOf(row())).toEqual({ key: MIN_LENGTH, value: 12 });
    expect(valuesOf(row({ value: undefined, default: 8 }))).toEqual({ key: MIN_LENGTH, value: 8 });
    expect(isSettingRow(row())).toBe(true);
    expect(isSettingRow({ key: MIN_LENGTH })).toBe(false);
  });
});
