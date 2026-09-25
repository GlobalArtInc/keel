import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor } from '@globalart/keel/meta';
import { KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const DUMP = resolve(__dirname, '../../../../../contracts/dump');
const STREAM = 'keel.logs.stream';

const DESCRIPTOR = JSON.parse(
  readFileSync(join(DUMP, 'admin/functions', `${STREAM}.json`), 'utf8'),
) as Descriptor;
const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;

const MESSAGES = {
  keel: {
    logs: { stream: { title: 'Log' } },
    stream: {
      live: 'Live',
      ended: 'The stream has ended.',
      failed: 'The stream could not be opened or was interrupted.',
      reconnect: 'Reconnect',
    },
  },
};

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  fixture.detectChanges();
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

function texts(element: HTMLElement): string[] {
  return [...element.querySelectorAll('phx-log-view .line .text')].map(
    (line) => line.textContent?.trim() ?? '',
  );
}

describe('keel-stream', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('the_stream_renderer_follows_and_pauses', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [DESCRIPTOR],
      catalogue: { lang: 'en', messages: MESSAGES },
    });
    TestBed.configureTestingModule({
      providers: [
        providePhoenixIcons(),
        provideRouter(
          [{ path: 'f/:func', component: KeelScreenComponent }],
          withComponentInputBinding(),
        ),
        { provide: KeelClient, useValue: client },
      ],
    });
    await TestBed.inject(KeelBoot).load();
    await TestBed.inject(KeelI18n).load('en');
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl(`/f/${STREAM}`, KeelScreenComponent);
    await settle(harness.fixture);
    const element = harness.routeNativeElement;
    if (element === null) {
      throw new Error('the screen did not render');
    }

    const host = element.querySelector(`keel-stream[data-stream="${STREAM}"]`);
    expect(host?.getAttribute('data-phase')).toBe('live');
    expect(client.streams).toEqual([{ func: STREAM, params: {} }]);
    expect(element.querySelector('phx-page-header')?.textContent).toContain('Log');
    expect(element.querySelector('phx-badge')?.textContent?.trim()).toBe('Live');
    expect(element.querySelector('phx-log-view [role="log"]')?.getAttribute('aria-label')).toBe(
      'Log',
    );

    const feed = client.feed(STREAM);
    feed.next({ at: '2026-09-26T10:00:00Z', level: 'info', text: 'one' });
    feed.next({ level: 'warn', text: 'two' });
    await settle(harness.fixture);
    expect(texts(element)).toEqual(['one', 'two']);
    feed.next({ level: 'error', text: 'three' });
    await settle(harness.fixture);
    expect(texts(element)).toEqual(['one', 'two', 'three']);

    const toggle = element.querySelector<HTMLButtonElement>('phx-log-view button.toggle');
    toggle?.click();
    await settle(harness.fixture);
    expect(toggle?.getAttribute('aria-pressed')).toBe('true');
    feed.next({ level: 'info', text: 'four' });
    await settle(harness.fixture);
    expect(texts(element)).toEqual(['one', 'two', 'three']);
    expect(toggle?.querySelector('.pending')?.textContent?.trim()).toBe('+1');

    toggle?.click();
    await settle(harness.fixture);
    expect(toggle?.getAttribute('aria-pressed')).toBe('false');
    expect(texts(element)).toEqual(['one', 'two', 'three', 'four']);

    feed.complete();
    await settle(harness.fixture);
    expect(host?.getAttribute('data-phase')).toBe('ended');
    expect(element.querySelector('phx-banner')?.textContent).toContain('The stream has ended.');
    expect(texts(element)).toEqual(['one', 'two', 'three', 'four']);

    element.querySelector<HTMLButtonElement>('[data-action="reconnect"]')?.click();
    await settle(harness.fixture);
    expect(client.streams).toHaveLength(2);
    expect(host?.getAttribute('data-phase')).toBe('live');
    expect(texts(element)).toEqual([]);

    client.feed(STREAM).error(new Error('the stream closed'));
    await settle(harness.fixture);
    expect(host?.getAttribute('data-phase')).toBe('failed');
    expect(element.querySelector('phx-banner')?.textContent).toContain(
      'The stream could not be opened or was interrupted.',
    );
  });
});
