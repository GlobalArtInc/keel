import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import { CONTROLS, KeelFieldComponent, buildForm, type FormField } from '@globalart/keel/forms';
import type { Control, Descriptor } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const MESSAGES = {
  demo: {
    note: {
      value: 'Value',
      'value.hint': 'How the value is used',
      'value.hint.A': 'IPv4 address only',
      'value.placeholder': 'Type here',
      'value.on': 'Switched on',
      'value.off': 'Switched off',
      'value.dns': 'DNS',
      'value.mail': 'Mail',
      'value.web': 'Web',
      page: { main: 'Main' },
    },
    brand: { hue: 'Hue', 'hue.empty': 'From the product', chroma: 'Chroma' },
  },
  keel: { schema: { required: 'Required' } },
  phx: { unlimited: 'No limit' },
};

const COMPONENT: Record<Control, string> = {
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

const SCHEMAS: Partial<Record<Control, Record<string, unknown>>> = {
  list: { type: 'array', items: { type: 'object', properties: { host: { type: 'string' } } } },
  switcher: { type: 'string', enum: ['on', 'off'] },
  key_value: { type: 'object', additionalProperties: { type: 'string' } },
  transfer: { type: 'array', items: { type: 'string', enum: ['dns', 'mail', 'web'] } },
};

function descriptorWith(property: Record<string, unknown>, required = false): Descriptor {
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
      required: required ? ['value'] : [],
    },
    result: { type: 'object' },
    ui: { title: 'demo.note.edit.title', pages: [{ id: 'main', fields: ['value'] }] },
  };
}

function fieldOf(
  property: Record<string, unknown>,
  values: Record<string, unknown> = {},
  required = false,
): FormField {
  const field = buildForm(descriptorWith(property, required), { values }).field('value');
  if (field === undefined) {
    throw new Error('the synthetic descriptor has no field value');
  }
  return field;
}

function brandFieldOf(name: string): FormField {
  const descriptor: Descriptor = {
    ...descriptorWith({}),
    name: 'demo.brand.set',
    params: {
      type: 'object',
      properties: {
        hue: { type: ['integer', 'null'], minimum: 0, maximum: 359 },
        chroma: { type: ['number', 'null'], minimum: 0, maximum: 0.4 },
      },
    },
    ui: { title: 'demo.brand.set.title', pages: [{ id: 'main', fields: ['hue', 'chroma'] }] },
  };
  const field = buildForm(descriptor).field(name);
  if (field === undefined) {
    throw new Error(`the brand descriptor has no field ${name}`);
  }
  return field;
}

async function render(
  field: FormField,
  client = new KeelMockClient({ catalogue: { lang: 'en', messages: MESSAGES } }),
): Promise<ComponentFixture<KeelFieldComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelFieldComponent],
    providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelFieldComponent);
  fixture.componentRef.setInput('field', field);
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  return fixture;
}

function typeInto(input: HTMLInputElement | undefined, text: string): void {
  if (input === undefined) {
    throw new Error('no input to type into');
  }
  input.value = text;
  input.dispatchEvent(new Event('input'));
}

function button(fixture: ComponentFixture<KeelFieldComponent>, label: string): HTMLButtonElement {
  const found = [...element(fixture).querySelectorAll<HTMLButtonElement>('button')].find(
    (candidate) =>
      candidate.getAttribute('aria-label') === label || candidate.textContent?.trim() === label,
  );
  if (found === undefined) {
    throw new Error(`no button ${label}`);
  }
  return found;
}

function checkbox(fixture: ComponentFixture<KeelFieldComponent>, side: string, label: string) {
  const group = [...element(fixture).querySelectorAll('phx-transfer-list section')].find(
    (section) => section.querySelector('.side__heading')?.textContent?.trim() === side,
  );
  return [...(group?.querySelectorAll<HTMLLabelElement>('phx-checkbox label') ?? [])]
    .find((candidate) => candidate.textContent?.trim() === label)
    ?.querySelector<HTMLInputElement>('input');
}

function labelsOf(fixture: ComponentFixture<KeelFieldComponent>, side: string): string[] {
  const group = [...element(fixture).querySelectorAll('phx-transfer-list section')].find(
    (section) => section.querySelector('.side__heading')?.textContent?.trim() === side,
  );
  return [...(group?.querySelectorAll('phx-checkbox label') ?? [])].map(
    (label) => label.textContent?.trim() ?? '',
  );
}

function element(fixture: ComponentFixture<KeelFieldComponent>): HTMLElement {
  return fixture.nativeElement;
}

describe('keel-field', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('renders each of the fourteen controls on a phx-form-row with the translated label', async () => {
    for (const control of CONTROLS) {
      const schema = SCHEMAS[control] ?? { type: 'string' };
      const fixture = await render(fieldOf({ ...schema, 'x-ui': { control } }));
      const row = element(fixture).querySelector('phx-form-row');
      expect(row).not.toBeNull();
      expect(row?.querySelector('.label__text')?.textContent).toBe('Value');
      expect(row?.querySelector(COMPONENT[control])).not.toBeNull();
      TestBed.resetTestingModule();
    }
  });

  it('a hint and a placeholder come from the keys x-ui names', async () => {
    const fixture = await render(
      fieldOf({
        type: 'string',
        'x-ui': { help: 'demo.note.value.hint', placeholder: 'demo.note.value.placeholder' },
      }),
    );
    expect(
      element(fixture).querySelector('phx-field-hint .anchor')?.getAttribute('aria-label'),
    ).toBe('Help');
    expect(element(fixture).querySelector('phx-input input')?.getAttribute('placeholder')).toBe(
      'Type here',
    );
  });

  it('a hint with checkBy follows the value of its source field and falls back to the general hint', async () => {
    const descriptor: Descriptor = {
      ...descriptorWith({}),
      params: {
        type: 'object',
        properties: {
          type: { type: 'string' },
          value: {
            type: 'string',
            'x-keel': { checkBy: { field: 'type', cases: { A: 'ipv4' } } },
            'x-ui': { help: 'demo.note.value.hint' },
          },
        },
        required: [],
      },
    };
    const form = buildForm(descriptor, { values: { type: 'A' } });
    const field = form.field('value');
    if (field === undefined) {
      throw new Error('the synthetic descriptor has no field value');
    }
    const fixture = await render(field);
    expect(fixture.componentInstance['hint']()).toBe('IPv4 address only');
    form.group.controls['type'].setValue('MX');
    fixture.detectChanges();
    expect(fixture.componentInstance['hint']()).toBe('How the value is used');
  });

  it('a required field marks its row and a plain one does not', async () => {
    const marked = await render(fieldOf({ type: 'string' }, {}, true));
    expect(element(marked).querySelector('phx-form-row .label__asterisk')).not.toBeNull();
    TestBed.resetTestingModule();
    const plain = await render(fieldOf({ type: 'string' }));
    expect(element(plain).querySelector('phx-form-row .label__asterisk')).toBeNull();
  });

  it('a select without a control lists the enum with translated labels', async () => {
    const fixture = await render(
      fieldOf({ type: 'string', enum: ['on', 'off'] }, { value: 'off' }),
    );
    const trigger = element(fixture).querySelector('phx-select .trigger');
    expect(trigger?.textContent?.trim()).toBe('Switched off');
  });

  it('a switcher offers the enum as radio buttons and writes the control', async () => {
    const field = fieldOf({ type: 'string', enum: ['on', 'off'], 'x-ui': { control: 'switcher' } });
    const fixture = await render(field);
    const buttons = element(fixture).querySelectorAll<HTMLButtonElement>('phx-switcher button');
    expect([...buttons].map((button) => button.textContent?.trim())).toEqual([
      'Switched on',
      'Switched off',
    ]);
    buttons[1].click();
    expect(field.control.value).toBe('off');
  });

  it('a slider carries a phx-number-field beside it and both write the control', async () => {
    const field = fieldOf(
      { type: ['integer', 'null'], minimum: 1, maximum: 10, 'x-ui': { control: 'slider' } },
      { value: 4 },
    );
    const fixture = await render(field);
    const slider = element(fixture).querySelector<HTMLInputElement>('phx-slider input[type=range]');
    const number = element(fixture).querySelector<HTMLInputElement>('phx-number-field input');
    expect(slider?.value).toBe('4');
    expect(number?.value).toBe('4');

    if (number === null) {
      throw new Error('no number field');
    }
    number.value = '7';
    number.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    expect(field.control.value).toBe(7);
    expect(field.control.dirty).toBe(true);
    expect(slider?.value).toBe('7');

    field.control.setValue(null);
    fixture.detectChanges();
    expect(number.value).toBe('No limit');
    expect(slider?.disabled).toBe(true);
  });

  it('a date field reads and writes the wire date', async () => {
    const field = fieldOf({ type: 'string', format: 'date' }, { value: '2026-09-13' });
    const fixture = await render(field);
    const input = element(fixture).querySelector<HTMLInputElement>('phx-date-field input');
    expect(input?.value).toBe('09/13/2026');

    if (input === null) {
      throw new Error('no date input');
    }
    input.value = '2026-10-01';
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new Event('blur'));
    fixture.detectChanges();
    expect(field.control.value).toBe('2026-10-01');
    expect(field.control.touched).toBe(true);
  });

  it('a toggle shows its finding on a phx-field-error under the control', async () => {
    const field = fieldOf({ type: 'boolean', 'x-ui': { control: 'toggle' } }, {}, true);
    const fixture = await render(field);
    expect(element(fixture).querySelector('phx-field-error')).toBeNull();

    field.control.setValue(null);
    field.control.markAsTouched();
    fixture.detectChanges();
    expect(element(fixture).querySelector('phx-field-error .text')?.textContent?.trim()).toBe(
      'Required',
    );
    expect(element(fixture).querySelector('phx-toggle input')?.getAttribute('aria-invalid')).toBe(
      'true',
    );
  });

  it('a password with generate offers the generate control', async () => {
    const fixture = await render(
      fieldOf({ type: 'string', 'x-keel': { secret: true }, 'x-ui': { generate: true } }),
    );
    expect(element(fixture).querySelector('phx-password [icon="dices"]')).not.toBeNull();
    TestBed.resetTestingModule();
    const plain = await render(fieldOf({ type: 'string', 'x-keel': { secret: true } }));
    expect(element(plain).querySelector('phx-password [icon="dices"]')).toBeNull();
  });

  it('control file renders phx-file-field with accept and maxSize from x-keel.file', async () => {
    const field = fieldOf({
      type: 'string',
      'x-ui': { control: 'file' },
      'x-keel': { file: { maxBytes: 65536, types: ['image/svg+xml', 'image/png'] } },
    });
    expect(field.mapping).toEqual({
      component: 'phx-file-field',
      accept: ['image/svg+xml', 'image/png'],
      maxSize: 65536,
      preset: null,
    });
    const fixture = await render(field);
    const file = element(fixture).querySelector('phx-file-field');
    expect(file).not.toBeNull();
    const input = element(fixture).querySelector<HTMLInputElement>(
      'phx-file-field input[type=file]',
    );
    expect(input?.getAttribute('accept')).toBe('image/svg+xml,image/png');
    expect(fixture.componentInstance['maxSize']()).toBe(65536);
    TestBed.resetTestingModule();

    const bare = fieldOf({ type: 'string', 'x-ui': { control: 'file' } });
    expect(bare.mapping).toEqual({
      component: 'phx-file-field',
      accept: [],
      maxSize: null,
      preset: null,
    });
    const plain = await render(bare);
    expect(
      element(plain)
        .querySelector<HTMLInputElement>('phx-file-field input[type=file]')
        ?.getAttribute('accept'),
    ).toBeNull();
  });

  it('a nullable number shows the field\u2019s .empty label through the scoped PHX_LABELS when the catalogue has it and phx.unlimited otherwise', async () => {
    const hue = await render(brandFieldOf('hue'));
    const number = element(hue).querySelector<HTMLInputElement>('phx-number-field input');
    expect(number?.value).toBe('From the product');
    expect(hue.componentInstance.labels.unlimited).toBe('From the product');
    expect(hue.componentInstance.labels.cancel).toBe('Cancel');
    TestBed.resetTestingModule();

    const chroma = await render(brandFieldOf('chroma'));
    const plain = element(chroma).querySelector<HTMLInputElement>('phx-number-field input');
    expect(plain?.value).toBe('No limit');
    expect(chroma.componentInstance.labels.unlimited).toBe('No limit');
  });

  it('a_code_widget_renders_the_code_editor', async () => {
    const text = 'server {\n  listen 80;\n}';
    const field = fieldOf({ type: 'string', 'x-ui': { control: 'code' } }, { value: text });
    const fixture = await render(field);
    const editor = element(fixture).querySelector('phx-form-row phx-code-editor');
    expect(editor).not.toBeNull();
    expect(element(fixture).querySelector('phx-code-field')).toBeNull();
    const lines = [...(editor?.querySelectorAll('.cm-content .cm-line') ?? [])].map(
      (line) => line.textContent,
    );
    expect(lines).toEqual(['server {', '  listen 80;', '}']);
    expect(editor?.querySelector('.cm-content')?.getAttribute('contenteditable')).toBe('true');
    expect(field.control.value).toBe(text);
  });

  it('a_key_value_widget_edits_a_map', async () => {
    const field = fieldOf(
      {
        type: 'object',
        additionalProperties: { type: 'string' },
        propertyNames: { pattern: '^[a-z]+$' },
        'x-ui': { control: 'key_value' },
      },
      { value: { env: 'prod' } },
    );
    const fixture = await render(field);
    const keys = () =>
      [...element(fixture).querySelectorAll<HTMLInputElement>('phx-key-value-field input')].filter(
        (input) => input.getAttribute('aria-label')?.startsWith('Key'),
      );
    const values = () =>
      [...element(fixture).querySelectorAll<HTMLInputElement>('phx-key-value-field input')].filter(
        (input) => input.getAttribute('aria-label')?.startsWith('Value'),
      );
    expect(keys().map((input) => input.value)).toEqual(['env']);
    expect(values().map((input) => input.value)).toEqual(['prod']);

    button(fixture, 'Add').click();
    fixture.detectChanges();
    typeInto(keys()[1], 'tier');
    typeInto(values()[1], 'web');
    fixture.detectChanges();
    expect(field.control.value).toEqual({ env: 'prod', tier: 'web' });
    expect(field.control.valid).toBe(true);

    typeInto(keys()[1], 'env');
    fixture.detectChanges();
    expect(field.control.value).toEqual({ env: 'prod' });
    expect(field.control.valid).toBe(false);
    expect(
      element(fixture).querySelector('phx-key-value-field phx-field-error')?.textContent,
    ).toContain('This key is already used');

    typeInto(keys()[1], 'Tier');
    fixture.detectChanges();
    expect(field.control.valid).toBe(false);
    expect(
      element(fixture).querySelector('phx-key-value-field phx-field-error')?.textContent,
    ).toContain('This key is not allowed');

    button(fixture, 'Remove Tier').click();
    button(fixture, 'Remove env').click();
    fixture.detectChanges();
    expect(field.control.value).toEqual({});
    expect(field.control.valid).toBe(true);
  });

  it('a_transfer_widget_edits_a_list_of_ids', async () => {
    const field = fieldOf(
      {
        type: 'array',
        items: { type: 'string', enum: ['dns', 'mail', 'web'] },
        'x-ui': { control: 'transfer' },
      },
      { value: ['mail'] },
    );
    const fixture = await render(field);
    expect(labelsOf(fixture, 'Available')).toEqual(['DNS', 'Web']);
    expect(labelsOf(fixture, 'Chosen')).toEqual(['Mail']);

    checkbox(fixture, 'Available', 'Web')?.click();
    fixture.detectChanges();
    button(fixture, 'Move selected to chosen').click();
    fixture.detectChanges();
    expect(field.control.value).toEqual(['mail', 'web']);
    expect(labelsOf(fixture, 'Chosen')).toEqual(['Mail', 'Web']);

    checkbox(fixture, 'Chosen', 'Mail')?.click();
    fixture.detectChanges();
    button(fixture, 'Move selected to available').click();
    fixture.detectChanges();
    expect(field.control.value).toEqual(['web']);
    expect(labelsOf(fixture, 'Available')).toEqual(['DNS', 'Mail']);
  });

  it('a transfer widget with an options function lists the choices the function answers', async () => {
    const client = new KeelMockClient({
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'dns.zone.options': {
          ok: true,
          data: {
            data: [
              { value: 'z1', label: 'example.com' },
              { value: 'z2', label: 'example.net', hint: 'secondary' },
            ],
            more: false,
          },
          meta: { requestId: 'req-options' },
        },
      },
    });
    const descriptor = descriptorWith({
      type: 'array',
      items: { type: 'string' },
      'x-keel': { options: 'dns.zone.options' },
      'x-ui': { control: 'transfer' },
    });
    const field = buildForm(descriptor, { client, values: { value: ['z2'] } }).field('value');
    if (field === undefined) {
      throw new Error('no field value');
    }
    const fixture = await render(field, client);
    expect(client.calls.map((call) => call.func)).toEqual(['dns.zone.options']);
    expect(labelsOf(fixture, 'Available')).toEqual(['example.com']);
    expect(labelsOf(fixture, 'Chosen')).toEqual(['example.netsecondary']);
  });

  it('a readonly field renders its control disabled', async () => {
    const descriptor = descriptorWith({ type: 'string' });
    descriptor.ui = {
      ...descriptor.ui,
      title: 'demo.note.edit.title',
      fields: { value: { readonly_if: { eq: ['$mode', 'edit'] } } },
    };
    const field = buildForm(descriptor, { mode: 'edit', values: { value: 'kept' } }).field('value');
    if (field === undefined) {
      throw new Error('no field value');
    }
    const fixture = await render(field);
    const input = element(fixture).querySelector<HTMLInputElement>('phx-input input');
    expect(input?.disabled).toBe(true);
    expect(input?.value).toBe('kept');
  });
});
