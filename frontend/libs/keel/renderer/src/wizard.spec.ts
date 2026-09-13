import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { Location } from '@angular/common';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { provideLocationMocks } from '@angular/common/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { buildForm, type Saved } from '@globalart/keel/forms';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import {
  KeelScreenComponent,
  KeelWizardComponent,
  fieldsUpTo,
  firstInvalidPage,
  stepsOf,
} from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const SNAPSHOTS = resolve(__dirname, '../../../../../products/demo/tests/snapshots');
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function snapshot(func: string, level: string): Descriptor {
  const text = readFileSync(join(SNAPSHOTS, `${func}@${level}.snap`), 'utf8');
  const body = text.slice(text.indexOf('\n---\n', text.indexOf('---\n') + 4) + 5);
  return JSON.parse(body) as Descriptor;
}

const IMPORT = snapshot('note.import', 'user');
const BOOT = load<Boot>('boot.json');
const TEXT = 'first line\nsecond line';
const FIXED = 'first line\nshorter';

const MESSAGES = {
  note: {
    import: { title: 'Import notes' },
    text: 'Lines',
    kind: 'Kind',
    tags: 'Tags',
    page: { source: 'Source', options: 'Options' },
    error: {
      line_too_long: 'Line {line} is longer than {max} characters',
      limit: 'An account keeps at most {max} notes',
    },
  },
  keel: {
    action: { back: 'Back', next: 'Next', finish: 'Finish', retry: 'Retry' },
    schema: { required: 'The field {property} is required' },
    error: { request: '{message} (request {id})' },
  },
};

function ok(data: unknown, requestId = 'req-ok'): Envelope {
  return { ok: true, data, meta: { requestId } };
}

function refused(path: string, key: string, params: Record<string, unknown>): Envelope {
  return {
    ok: false,
    error: {
      code: 'validation',
      key: 'keel.error.validation',
      requestId: 'req-refused',
      fields: [{ path, code: 'validation', key, params }],
    },
  };
}

function mock(answers: Record<string, Envelope | Envelope[]>): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: [IMPORT],
    catalogue: { lang: 'en', messages: MESSAGES },
    answers,
  });
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

async function render(client: KeelMockClient): Promise<ComponentFixture<KeelWizardComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelWizardComponent],
    providers: [
      providePhoenixIcons(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      provideLocationMocks(),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelWizardComponent);
  fixture.componentRef.setInput('descriptor', IMPORT);
  fixture.detectChanges();
  await settle(fixture);
  return fixture;
}

function must<T extends Element>(node: T | null | undefined, what: string): T {
  if (node === null || node === undefined) {
    throw new Error(`no ${what}`);
  }
  return node;
}

function steps(element: HTMLElement): [string, boolean, boolean][] {
  return [...element.querySelectorAll<HTMLButtonElement>('phx-wizard .step')].map((step) => [
    step.textContent?.trim() ?? '',
    step.classList.contains('step_active'),
    step.disabled,
  ]);
}

function type(element: HTMLElement, text: string): void {
  const area = must(element.querySelector<HTMLTextAreaElement>('phx-code-field textarea'), 'text');
  area.value = text;
  area.dispatchEvent(new Event('input'));
}

function fieldError(element: HTMLElement): string | null {
  return element.querySelector('phx-code-field phx-field-error .text')?.textContent?.trim() ?? null;
}

describe('the wizard renderer', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('a step is validated on the server before the next opens', async () => {
    const client = mock({
      'note.import.check': [
        refused('text', 'note.error.line_too_long', { line: 2, max: 200 }),
        ok({ lines: 2 }),
      ],
    });
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-wizard')).toBe('note.import');
    expect(element.getAttribute('data-phase')).toBe('ready');
    expect(element.getAttribute('data-step')).toBe('source');
    expect(element.querySelector('phx-form-sheet h1')?.textContent).toBe('Import notes');
    expect(steps(element)).toEqual([
      ['1Source', true, false],
      ['2Options', false, true],
    ]);
    expect(element.querySelector('keel-page')?.getAttribute('data-page')).toBe('source');
    expect(element.querySelectorAll('keel-page')).toHaveLength(1);
    const back = must(element.querySelector<HTMLButtonElement>('button.back'), 'back');
    const next = must(element.querySelector<HTMLButtonElement>('button.next'), 'next');
    expect(back.disabled).toBe(true);
    expect(next.textContent?.trim()).toBe('Next');

    next.click();
    await settle(fixture);
    expect(client.calls).toHaveLength(0);
    expect(fieldError(element)).toBe('The field text is required');
    expect(element.getAttribute('data-step')).toBe('source');

    type(element, TEXT);
    next.click();
    await settle(fixture);
    expect(client.calls.map((call) => [call.func, call.params])).toEqual([
      ['note.import.check', { text: TEXT }],
    ]);
    expect(element.getAttribute('data-step')).toBe('source');
    expect(fieldError(element)).toBe('Line 2 is longer than 200 characters');
    expect(element.querySelector('keel-findings')).toBeNull();

    next.click();
    await settle(fixture);
    expect(client.calls).toHaveLength(1);
    expect(element.getAttribute('data-step')).toBe('source');

    type(element, FIXED);
    fixture.detectChanges();
    expect(fieldError(element)).toBeNull();
    next.click();
    await settle(fixture);
    expect(client.calls).toHaveLength(2);
    expect(client.calls[1].params).toEqual({ text: FIXED });
    expect(element.getAttribute('data-step')).toBe('options');
    expect(steps(element)).toEqual([
      ['Source', false, false],
      ['2Options', true, false],
    ]);
    expect(element.querySelector('keel-page')?.getAttribute('data-page')).toBe('options');
    expect(element.querySelector('keel-page phx-select')).not.toBeNull();
    expect(element.querySelector('keel-page phx-tags-field')).not.toBeNull();
    expect(element.querySelector('button.next')?.textContent?.trim()).toBe('Finish');
    expect(back.disabled).toBe(false);

    back.click();
    await settle(fixture);
    expect(element.getAttribute('data-step')).toBe('source');
    expect(element.querySelector<HTMLTextAreaElement>('phx-code-field textarea')?.value).toBe(
      FIXED,
    );
    expect(client.calls).toHaveLength(2);

    next.click();
    await settle(fixture);
    expect(element.getAttribute('data-step')).toBe('options');
    must(element.querySelectorAll<HTMLButtonElement>('phx-wizard .step')[0], 'source step').click();
    await settle(fixture);
    expect(element.getAttribute('data-step')).toBe('source');
    fixture.destroy();
  });

  it('the last step submits every page and redirect navigates back', async () => {
    const client = mock({
      'note.import.check': ok({ lines: 2 }),
      'note.import': ok({ imported: 2, ids: ['n1', 'n2'] }, 'req-imported'),
    });
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    const back = jest.spyOn(TestBed.inject(Location), 'back');
    const done: Saved[] = [];
    fixture.componentInstance.done.subscribe((saved) => done.push(saved));

    type(element, TEXT);
    must(element.querySelector<HTMLButtonElement>('button.next'), 'next').click();
    await settle(fixture);
    expect(element.getAttribute('data-step')).toBe('options');

    must(element.querySelector<HTMLButtonElement>('button.next'), 'finish').click();
    await settle(fixture);
    expect(client.calls.map((call) => call.func)).toEqual(['note.import.check', 'note.import']);
    expect(client.calls[1].params).toEqual({ text: TEXT, kind: 'memo', tags: [] });
    expect(client.calls[1].options.idempotencyKey).toMatch(UUID);
    expect(done).toHaveLength(1);
    expect(done[0].data).toEqual({ imported: 2, ids: ['n1', 'n2'] });
    expect(back).toHaveBeenCalledTimes(1);
    fixture.destroy();
  });

  it('a refusal of the final submit reopens the page carrying the field and shows the rest on the banner', async () => {
    const client = mock({
      'note.import.check': ok({ lines: 2 }),
      'note.import': [
        refused('text', 'note.error.line_too_long', { line: 1, max: 200 }),
        {
          ok: false,
          error: {
            code: 'limit_exceeded',
            key: 'note.error.limit',
            params: { max: 100 },
            requestId: 'req-limit',
          },
        },
      ],
    });
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    const back = jest.spyOn(TestBed.inject(Location), 'back');

    type(element, TEXT);
    must(element.querySelector<HTMLButtonElement>('button.next'), 'next').click();
    await settle(fixture);
    must(element.querySelector<HTMLButtonElement>('button.next'), 'finish').click();
    await settle(fixture);
    expect(element.getAttribute('data-step')).toBe('source');
    expect(fieldError(element)).toBe('Line 1 is longer than 200 characters');
    expect(back).not.toHaveBeenCalled();

    type(element, 'fixed');
    must(element.querySelector<HTMLButtonElement>('button.next'), 'next').click();
    await settle(fixture);
    must(element.querySelector<HTMLButtonElement>('button.next'), 'finish').click();
    await settle(fixture);
    expect(element.getAttribute('data-step')).toBe('options');
    expect(element.querySelector('keel-findings .banner__message')?.textContent).toBe(
      'An account keeps at most 100 notes',
    );
    expect(client.callsOf('note.import').map((call) => call.options.idempotencyKey)).toEqual([
      expect.stringMatching(UUID),
      expect.stringMatching(UUID),
    ]);
    fixture.destroy();
  });

  it('keel-screen renders a wizard descriptor on keel-wizard', async () => {
    const client = mock({});
    TestBed.configureTestingModule({
      imports: [KeelScreenComponent],
      providers: [
        providePhoenixIcons(),
        provideRouter([{ path: 'f/:func', children: [] }]),
        provideLocationMocks(),
        { provide: KeelClient, useValue: client },
      ],
    });
    await TestBed.inject(KeelBoot).load();
    await TestBed.inject(KeelI18n).load('en');
    const fixture = TestBed.createComponent(KeelScreenComponent);
    fixture.componentRef.setInput('func', 'note.import');
    fixture.detectChanges();
    await settle(fixture);
    await settle(fixture);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('screen');
    expect(element.querySelector('keel-wizard')?.getAttribute('data-step')).toBe('source');
    expect(element.querySelector('phx-not-found')).toBeNull();
    fixture.destroy();
  });

  it('stepsOf disables the steps after the reached one and fieldsUpTo collects the pages so far', () => {
    const form = buildForm(IMPORT);
    const pages = form.visiblePages();
    expect(stepsOf(pages, 0, (page) => page.labelKey)).toEqual([
      { id: 'source', label: 'note.page.source', disabled: false },
      { id: 'options', label: 'note.page.options', disabled: true },
    ]);
    expect(stepsOf(pages, 1, (page) => page.id)).toEqual([
      { id: 'source', label: 'source', disabled: false },
      { id: 'options', label: 'options', disabled: false },
    ]);
    expect(fieldsUpTo(pages, 0).map((field) => field.name)).toEqual(['text']);
    expect(fieldsUpTo(pages, 1).map((field) => field.name)).toEqual(['text', 'kind', 'tags']);
    expect(firstInvalidPage(pages)).toBe(0);
    form.group.controls['text'].setValue('one');
    expect(firstInvalidPage(pages)).toBe(-1);
    form.destroy();
  });
});
