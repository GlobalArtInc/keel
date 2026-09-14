import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { KeelClient, KeelI18n, type Catalogue, type Fetched } from '@globalart/keel/client';
import { lookup, type Messages } from '@globalart/keel/meta';
import {
  LABELS_PREFIX,
  LABEL_NAMES,
  bridgeLabels,
  labelOf,
  provideKeelLabels,
} from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';
import { PHX_DEFAULT_LABELS, PHX_LABELS } from '@globalart/phoenix-ui';
import { of, type Observable } from 'rxjs';

const DICTIONARIES = resolve(__dirname, '../../../../../crates/kernel/i18n');

const CATALOGUES: Record<string, Messages> = {
  en: { phx: { close: 'Close it', search: 'Find' } },
  ru: { phx: { close: 'Close (ru)', search: 'Find (ru)', retry: 'Retry (ru)' } },
};

class SpeakingClient extends KeelMockClient {
  override catalogue(lang: string): Observable<Fetched<Catalogue> | null> {
    return of({ body: { lang, messages: CATALOGUES[lang] ?? {} }, etag: null });
  }
}

function dictionary(lang: string): Messages {
  return JSON.parse(readFileSync(join(DICTIONARIES, `${lang}.json`), 'utf8')) as Messages;
}

describe('the labels bridge', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('PHX_LABELS is filled from phx.* of the catalogue and switches with the language', async () => {
    TestBed.configureTestingModule({
      providers: [{ provide: KeelClient, useValue: new SpeakingClient() }, provideKeelLabels()],
    });
    const labels = TestBed.inject(PHX_LABELS);
    const i18n = TestBed.inject(KeelI18n);
    expect(labels.close).toBe(PHX_DEFAULT_LABELS.close);

    await i18n.load('en');
    expect(labels.close).toBe('Close it');
    expect(labels.search).toBe('Find');
    expect(labels.retry).toBe(PHX_DEFAULT_LABELS.retry);

    await i18n.load('ru');
    expect(labels.close).toBe('Close (ru)');
    expect(labels.search).toBe('Find (ru)');
    expect(labels.retry).toBe('Retry (ru)');
    expect(labels.cancel).toBe(PHX_DEFAULT_LABELS.cancel);
    expect(Object.keys(labels)).toEqual(Object.keys(PHX_DEFAULT_LABELS));
  });

  it('the bridge reads live, falls back to the default and invents no name', () => {
    let messages: Messages = CATALOGUES['en'] ?? {};
    const labels = bridgeLabels(() => messages);
    expect(labels.close).toBe('Close it');
    messages = CATALOGUES['ru'] ?? {};
    expect(labels.close).toBe('Close (ru)');
    expect(labelOf(messages, 'search')).toBe('Find (ru)');
    expect(labelOf({}, 'search')).toBe(PHX_DEFAULT_LABELS.search);
    expect(labels.perPage).toBe(PHX_DEFAULT_LABELS.perPage);
    expect(LABEL_NAMES).toEqual(Object.keys(PHX_DEFAULT_LABELS));
    expect(LABELS_PREFIX).toBe('phx');
    expect((labels as unknown as Record<string, unknown>)['nothing']).toBeUndefined();
  });

  it.each(['en', 'ru'])(
    'the kernel dictionary %s carries every key of PHX_DEFAULT_LABELS',
    (lang) => {
      const messages = dictionary(lang);
      const missing = LABEL_NAMES.filter(
        (name) => lookup(messages, `${LABELS_PREFIX}.${name}`) === undefined,
      );
      expect(missing).toEqual([]);
      for (const name of LABEL_NAMES) {
        const label = labelOf(messages, name);
        expect(label).toBe(lookup(messages, `${LABELS_PREFIX}.${name}`));
        expect(label).not.toBe('');
      }
    },
  );
});
