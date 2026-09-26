import { resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { provideRouter } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { provideKeelPanel } from '@globalart/keel-panel';
import type { CallError, Envelope } from '@globalart/keel/meta';
import { KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient, fixturesOf, readDump } from '@globalart/keel/testing';
import { PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';
import { ConfigEditorComponent } from './config-editor/config-editor.component';
import { NO_ERRORS, partErrorsOf, type SiteConfig } from './config-editor/config';

const DUMP = resolve(__dirname, '../../../../contracts/dump');
const VIEW = 'hosting.site.config-editor';
const SITE = '01J9ZX0000000000000000SITE';
const DOMAIN = 'config.example.net';
const BLOCK = 'server {\n    server_name config.example.net;\n}\n';
const POOL = '[keel-panel-config.example.net]\nuser = hp_alice\n';
const OVERRIDE = 'client_max_body_size 64m;';
const RENDERED = `server {\n    server_name config.example.net;\n    ${OVERRIDE}\n}\n`;
const OUTPUT = 'nginx: [emerg] unknown directive "keel_test_refused"';

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-config' } };
}

function failed(error: CallError): Envelope {
  return { ok: false, error };
}

function config(overrides: Partial<SiteConfig> = {}): Envelope {
  return ok({
    id: SITE,
    domain: DOMAIN,
    php: '8.4',
    server: '',
    pool: '',
    rendered: { server: BLOCK, pool: POOL },
    ...overrides,
  });
}

const SITES = ok({ data: [{ id: SITE, domain: DOMAIN }], total: 1 });
const REFUSED: CallError = {
  code: 'validation',
  key: 'keel.error.validation',
  fields: [{ path: 'server', key: 'host.error.validate_failed', params: { detail: OUTPUT } }],
};

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  for (let round = 0; round < 4; round++) {
    await fixture.whenStable();
    await new Promise((done) => setTimeout(done, 0));
    fixture.detectChanges();
  }
}

async function render(
  answers: Record<string, Envelope | Envelope[]>,
  params: Record<string, unknown> = { id: SITE },
): Promise<{ fixture: ComponentFixture<KeelScreenComponent>; client: KeelMockClient }> {
  const client = new KeelMockClient({ ...fixturesOf(readDump(DUMP, 'admin')), answers });
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [
      providePhoenixIcons(),
      provideKeelPanel(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', VIEW);
  fixture.componentRef.setInput('params', params);
  fixture.detectChanges();
  await settle(fixture);
  return { fixture, client };
}

function editor(fixture: ComponentFixture<unknown>): ConfigEditorComponent {
  return fixture.debugElement.query(By.directive(ConfigEditorComponent))
    .componentInstance as ConfigEditorComponent;
}

function text(element: HTMLElement, part: string, side: string): string {
  const lines = element.querySelectorAll(
    `[data-part="${part}"] phx-code-editor.${side} .cm-content .cm-line`,
  );
  return [...lines].map((line) => line.textContent ?? '').join('\n');
}

function click(target: Element | null): void {
  if (!(target instanceof HTMLElement)) {
    throw new Error('nothing to click');
  }
  target.click();
}

describe('the configuration editor of panel', () => {
  afterEach(() => {
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('maps the field errors of a refused save onto the server block and the pool', () => {
    expect(partErrorsOf(REFUSED)).toEqual({ ...NO_ERRORS, server: REFUSED.fields?.[0] });
    expect(partErrorsOf({ code: 'unavailable', key: 'keel.error.unavailable' })).toBeNull();
    expect(
      partErrorsOf({ ...REFUSED, fields: [{ path: 'id', key: 'keel.error.not_found' }] }),
    ).toBeNull();
  });

  it('shows the overrides of the site beside the rendered server block and pool', async () => {
    const { fixture, client } = await render({
      'hosting.site.list': SITES,
      'hosting.site.config.get': config({ server: OVERRIDE }),
    });
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('panel-config-editor')?.getAttribute('data-phase')).toBe('loaded');
    expect(element.querySelector('phx-page-header .title__text')?.textContent).toBe(
      'Site configuration',
    );
    expect(client.callsOf('hosting.site.config.get').map((call) => call.params)).toEqual([
      { id: SITE },
    ]);
    expect(text(element, 'server', 'override')).toBe(OVERRIDE);
    expect(text(element, 'server', 'rendered')).toBe(BLOCK);
    expect(text(element, 'pool', 'rendered')).toBe(POOL);
    expect(
      element
        .querySelector('[data-part="server"] phx-code-editor.rendered .cm-content')
        ?.getAttribute('aria-readonly'),
    ).toBe('true');
    expect(element.querySelector('[data-part="server"] .heading')?.textContent).toBe(
      'Server block',
    );
  });

  it('says the pool is not rendered for a site without PHP', async () => {
    const { fixture } = await render({
      'hosting.site.list': SITES,
      'hosting.site.config.get': config({ php: null, rendered: { server: BLOCK, pool: null } }),
    });
    const element: HTMLElement = fixture.nativeElement;
    expect(
      element.querySelector('[data-part="pool"] phx-code-editor.rendered .cm-placeholder')
        ?.textContent,
    ).toBe('The site runs no PHP, so no pool is rendered');
    expect(
      element.querySelector('[data-part="server"] phx-code-editor.rendered .cm-placeholder'),
    ).toBeNull();
  });

  it('saves the overrides and shows the block rendered with them', async () => {
    const { fixture, client } = await render({
      'hosting.site.list': SITES,
      'hosting.site.config.get': [
        config(),
        config({ server: OVERRIDE, rendered: { server: RENDERED, pool: POOL } }),
      ],
      'hosting.site.config.set': ok({ id: SITE, server: OVERRIDE, pool: '' }),
    });
    const element: HTMLElement = fixture.nativeElement;
    editor(fixture).overrides.server.setValue(OVERRIDE);
    click(element.querySelector('button.save'));
    await settle(fixture);
    expect(client.callsOf('hosting.site.config.set').map((call) => call.params)).toEqual([
      { id: SITE, server: OVERRIDE, pool: '' },
    ]);
    expect(client.callsOf('hosting.site.config.get')).toHaveLength(2);
    expect(text(element, 'server', 'rendered')).toContain(`    ${OVERRIDE}`);
    expect(document.body.textContent).toContain(`The configuration of ${DOMAIN} is saved`);
  });

  it('keeps the edited override and shows the output of the refused check on it', async () => {
    const { fixture, client } = await render({
      'hosting.site.list': SITES,
      'hosting.site.config.get': config(),
      'hosting.site.config.set': failed(REFUSED),
    });
    const element: HTMLElement = fixture.nativeElement;
    editor(fixture).overrides.server.setValue('keel_test_refused on;');
    click(element.querySelector('button.save'));
    await settle(fixture);
    expect(client.callsOf('hosting.site.config.get')).toHaveLength(1);
    expect(text(element, 'server', 'override')).toBe('keel_test_refused on;');
    const field = element.querySelector('[data-part="server"] phx-code-editor.override');
    expect(field?.textContent).toContain(OUTPUT);
    expect(field?.textContent).toContain('The configuration check failed');
    expect(text(element, 'server', 'rendered')).toBe(BLOCK);
  });
});
