import { TestBed } from '@angular/core/testing';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import { KeelPageComponent, buildForm, type FormPage } from '@globalart/keel/forms';
import type { Descriptor } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const DESCRIPTOR: Descriptor = {
  keel: 1,
  name: 'demo.note.edit',
  module: 'demo',
  kind: 'form',
  mode: 'write',
  access: { level: 'user' },
  params: {
    type: 'object',
    properties: {
      title: { type: 'string', 'x-ui': { order: 2 } },
      body: { type: 'string' },
      pinned: { type: 'boolean', 'x-ui': { order: 1 } },
      color: { type: 'string' },
    },
  },
  result: { type: 'object' },
  ui: {
    title: 'demo.note.edit.title',
    pages: [{ id: 'main', fields: ['title', 'body', 'pinned', 'color'] }],
    fields: { color: { show_if: { truthy: 'pinned' } } },
  },
};

describe('keel-page', () => {
  it('renders a keel-field per visible field in the order of x-ui.order then the page', async () => {
    const form = buildForm(DESCRIPTOR);
    const page = form.page('main') as FormPage;
    TestBed.configureTestingModule({
      imports: [KeelPageComponent],
      providers: [
        providePhoenixIcons(),
        {
          provide: KeelClient,
          useValue: new KeelMockClient({ catalogue: { lang: 'en', messages: {} } }),
        },
      ],
    });
    await TestBed.inject(KeelI18n).load('en');
    const fixture = TestBed.createComponent(KeelPageComponent);
    fixture.componentRef.setInput('page', page);
    fixture.detectChanges();
    const element: HTMLElement = fixture.nativeElement;
    const labels = () =>
      [...element.querySelectorAll('keel-field .label__text')].map((label) => label.textContent);
    expect(element.getAttribute('data-page')).toBe('main');
    expect(page.fields.map((field) => field.name)).toEqual(['pinned', 'title', 'body', 'color']);
    expect(labels()).toEqual(['demo.note.pinned', 'demo.note.title', 'demo.note.body']);

    form.group.controls['pinned'].setValue(true);
    fixture.detectChanges();
    expect(labels()).toEqual([
      'demo.note.pinned',
      'demo.note.title',
      'demo.note.body',
      'demo.note.color',
    ]);
  });
});
