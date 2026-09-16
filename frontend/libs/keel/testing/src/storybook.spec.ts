import { resolve } from 'node:path';
import { ApplicationInitStatus, Component, inject } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Router } from '@angular/router';
import { KEEL_EVENT_SOURCE, KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { PHX_LABELS } from '@globalart/phoenix-ui';
import {
  DUMP_LEVELS,
  KEEL_BOOT,
  KeelMockClient,
  QuietEventSource,
  descriptorOf,
  dumpsOf,
  keelDump,
  keelStoryClient,
  keelStoryProviders,
  mergeMessages,
  readDump,
  registerKeelDumps,
  withKeel,
  type KeelDecorator,
} from '@globalart/keel/testing';
import { firstValueFrom } from 'rxjs';

const DUMP = resolve(__dirname, '../../../../../contracts/dump');

@Component({ selector: 'keel-test-story', template: '<p>{{ title() }}</p>' })
class StoryComponent {
  private readonly i18n = inject(KeelI18n);

  title(): string {
    return this.i18n.t('note.list.title');
  }
}

async function applied(decorator: KeelDecorator): Promise<void> {
  const story = decorator(() => ({
    props: { shown: true },
    applicationConfig: { providers: [{ provide: 'own', useValue: 1 }] },
  }));
  expect(story.props).toEqual({ shown: true });
  const providers = story.applicationConfig?.providers ?? [];
  expect(providers.at(-1)).toEqual({ provide: 'own', useValue: 1 });
  TestBed.configureTestingModule({ providers });
  await TestBed.inject(ApplicationInitStatus).donePromise;
}

describe('the Storybook decorator', () => {
  beforeAll(() => {
    registerKeelDumps(DUMP_LEVELS.map((level) => readDump(DUMP, level)));
  });

  afterEach(() => TestBed.resetTestingModule());

  it('keelDump answers a registered level and refuses an unregistered one', () => {
    expect(keelDump('admin').boot.principal?.login).toBe('root');
    expect(keelDump('guest').boot.principal).toBeNull();
    expect(descriptorOf(keelDump('user'), 'note.list').kind).toBe('list');
    expect(() => descriptorOf(keelDump('guest'), 'note.list')).toThrow(
      'the dump of guest has no descriptor note.list',
    );
    const files = new Map<string, unknown>([['admin/boot.json', KEEL_BOOT.one]]);
    expect(dumpsOf(files).map((dump) => dump.level)).toEqual(['admin']);
    expect(dumpsOf(files)[0].descriptors).toEqual([]);
    expect(() => dumpsOf(new Map([['user/functions/note.list.json', {}]]))).toThrow(
      'the dump of user has no boot.json',
    );
  });

  it('the story client serves the dump of the level with the boot, the calls and the messages given', async () => {
    const client = keelStoryClient({
      level: 'user',
      boot: KEEL_BOOT.two,
      calls: { 'note.list': { ok: true, data: { data: [], total: 0 }, meta: { requestId: 'r' } } },
      messages: { menu: { dns: 'DNS' }, 'product.dns.brand': 'KeelDNS' },
    });
    expect(client).toBeInstanceOf(KeelMockClient);
    const boot = await firstValueFrom(client.boot());
    expect(boot?.body).toEqual(KEEL_BOOT.two);
    const descriptors = await firstValueFrom(client.descriptors());
    expect(descriptors).toEqual(keelDump('user').descriptors);
    const catalogue = await firstValueFrom(client.catalogue('en'));
    expect(catalogue?.body.lang).toBe('en');
    const messages = catalogue?.body.messages ?? {};
    expect(messages['menu']).toEqual({ system: 'System', demo: 'Demo', dns: 'DNS' });
    expect(messages['product.dns.brand']).toBe('KeelDNS');
    expect((await firstValueFrom(client.call('note.list'))).ok).toBe(true);
    expect(() => keelStoryClient({ level: 'admin', boot: undefined }).boot()).not.toThrow();
  });

  it('mergeMessages keeps the nested keys of both sides', () => {
    expect(mergeMessages({ a: { b: 'x', c: 'y' }, d: 'z' }, { a: { b: 'X' }, e: 'w' })).toEqual({
      a: { b: 'X', c: 'y' },
      d: 'z',
      e: 'w',
    });
    expect(mergeMessages({ a: { b: 'x' } }, { a: 'flat' })).toEqual({ a: 'flat' });
  });

  it('withKeel provides the mock client, the quiet events, the router, the labels and the loaded boot before the story', async () => {
    await applied(withKeel({ level: 'reseller' }));
    expect(TestBed.inject(KeelClient)).toBeInstanceOf(KeelMockClient);
    expect(TestBed.inject(KEEL_EVENT_SOURCE)('/events', {})).toBeInstanceOf(QuietEventSource);
    expect(TestBed.inject(KeelBoot).principal()?.login).toBe('res1');
    expect(TestBed.inject(KeelI18n).lang()).toBe('en');
    expect(TestBed.inject(PHX_LABELS).search).toBe('Search');
    expect(TestBed.inject(Router).config.at(-1)).toEqual({ path: '**', children: [] });
    const fixture = TestBed.createComponent(StoryComponent);
    fixture.detectChanges();
    expect(fixture.nativeElement.textContent).toBe('Notes');
  });

  it('the quiet event source records what opened it and lets a story drive frames and failures', () => {
    TestBed.configureTestingModule({ providers: keelStoryProviders() });
    QuietEventSource.reset();
    expect(QuietEventSource.last()).toBeNull();
    const source = TestBed.inject(KEEL_EVENT_SOURCE)('/api/v1/events', { withCredentials: true });
    expect(source).toBeInstanceOf(QuietEventSource);
    expect(QuietEventSource.last()).toBe(source);
    const quiet = QuietEventSource.last();
    if (quiet === null) {
      throw new Error('the source was not recorded');
    }
    expect(quiet.url).toBe('/api/v1/events');
    expect(quiet.init).toEqual({ withCredentials: true });
    expect(quiet.readyState).toBe(0);

    const seen: [string, unknown, string][] = [];
    for (const type of ['open', 'error', 'data.invalidate', 'task.progress']) {
      quiet.addEventListener(type, (event) => {
        const message = event as Partial<MessageEvent>;
        seen.push([event.type, message.data, message.lastEventId ?? '']);
      });
    }
    quiet.open();
    expect(quiet.readyState).toBe(1);
    quiet.emit('data.invalidate', { seq: 9 }, '9');
    quiet.emit('task.progress', { done: 1 });
    quiet.emitRaw('task.progress', '{not json');
    quiet.drop();
    expect(quiet.readyState).toBe(0);
    quiet.fail();
    expect(quiet.readyState).toBe(2);
    expect(seen).toEqual([
      ['open', undefined, ''],
      ['data.invalidate', '{"seq":9}', '9'],
      ['task.progress', '{"done":1}', ''],
      ['task.progress', '{not json', ''],
      ['error', undefined, ''],
      ['error', undefined, ''],
    ]);
    quiet.close();
    expect(quiet.readyState).toBe(2);
    QuietEventSource.reset();
    expect(QuietEventSource.opened).toHaveLength(0);
  });

  it('the story providers put the routes of the story before the catch-all', () => {
    const providers = keelStoryProviders({ routes: [{ path: 'f/:func', children: [] }] });
    TestBed.configureTestingModule({ providers });
    expect(TestBed.inject(Router).config.map((route) => route.path)).toEqual(['f/:func', '**']);
    expect(providers.length).toBeGreaterThan(4);
  });
});
