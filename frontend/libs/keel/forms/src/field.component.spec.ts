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
      'value.placeholder': 'Type here',
      'value.on': 'Switched on',
      'value.off': 'Switched off',
      page: { main: 'Main' },
    },
  },
  keel: { schema: { required: 'Required' } },
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
  code: 'phx-code-field',
  tags: 'phx-tags-field',
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

function element(fixture: ComponentFixture<KeelFieldComponent>): HTMLElement {
  return fixture.nativeElement;
}

describe('keel-field', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('renders each of the twelve controls on a phx-form-row with the translated label', async () => {
    for (const control of CONTROLS) {
      const schema =
        control === 'list'
          ? { type: 'array', items: { type: 'object', properties: { host: { type: 'string' } } } }
          : control === 'switcher'
            ? { type: 'string', enum: ['on', 'off'] }
            : { type: 'string' };
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
    expect(number.value).toBe('Unlimited');
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
