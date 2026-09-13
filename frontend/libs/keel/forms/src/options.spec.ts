import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelClient, KeelI18n, type CallOptions } from '@globalart/keel/client';
import {
  KeelFieldComponent,
  OPTIONS_LIMIT,
  RemoteOptions,
  buildForm,
  optionsQuery,
  toSelectOption,
  type FormField,
} from '@globalart/keel/forms';
import type { Choices, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';
import { Observable, type Subscriber } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const SCOPE = { id: '01J8A0000000000000000000Z1' };
const DEBOUNCE = 300;

const MESSAGES = {
  dns: { zone: { account: 'Account' } },
};

function page(values: string[], more: boolean): Envelope<Choices> {
  return {
    ok: true,
    data: { data: values.map((value) => ({ value, label: value.toUpperCase() })), more },
    meta: { requestId: 'req-options' },
  };
}

class DeferredClient extends KeelMockClient {
  private readonly waiting: Subscriber<Envelope>[] = [];

  override call<T = unknown>(
    func: string,
    params: unknown = {},
    options: CallOptions = {},
  ): Observable<Envelope<T>> {
    this.calls.push({ func, params, options });
    return new Observable<Envelope>((subscriber) => {
      this.waiting.push(subscriber);
      return () => {
        const index = this.waiting.indexOf(subscriber);
        if (index >= 0) {
          this.waiting.splice(index, 1);
        }
      };
    }) as Observable<Envelope<T>>;
  }

  reply(envelope: Envelope): void {
    const subscriber = this.waiting.shift();
    if (subscriber === undefined) {
      throw new Error('no call is waiting for an answer');
    }
    subscriber.next(envelope);
    subscriber.complete();
  }
}

function accountField(client: KeelClient): FormField {
  const field = buildForm(EDIT, { params: SCOPE, client }).field('account');
  if (field === undefined) {
    throw new Error('no field account');
  }
  return field;
}

async function render(
  field: FormField,
  client: KeelClient,
): Promise<ComponentFixture<KeelFieldComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelFieldComponent],
    providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelFieldComponent);
  fixture.componentRef.setInput('field', field);
  fixture.detectChanges();
  return fixture;
}

function popup(): HTMLElement {
  const element = document.querySelector<HTMLElement>('.cdk-overlay-container .popup');
  if (element === null) {
    throw new Error('the select popup is not open');
  }
  return element;
}

function optionLabels(): string[] {
  return [...popup().querySelectorAll('[role=option]')].map(
    (option) => option.textContent?.trim() ?? '',
  );
}

describe('options', () => {
  afterEach(() => {
    jest.useRealTimers();
    TestBed.resetTestingModule();
  });

  it('typing acme into account calls keel.account.options once after the debounce, fills the options and offers loadMore with a cursor', async () => {
    jest.useFakeTimers({ doNotFake: ['queueMicrotask', 'nextTick'] });
    const client = new DeferredClient({ catalogue: { lang: 'en', messages: MESSAGES } });
    const field = accountField(client);
    const options = field.options;
    if (options === null) {
      throw new Error('account has no remote options');
    }
    const fixture = await render(field, client);
    const element: HTMLElement = fixture.nativeElement;
    const trigger = element.querySelector<HTMLButtonElement>('phx-select .trigger');
    if (trigger === null) {
      throw new Error('no select trigger');
    }

    trigger.click();
    fixture.detectChanges();
    expect(client.callsOf('keel.account.options').map((call) => call.params)).toEqual([
      { scope: SCOPE, limit: OPTIONS_LIMIT },
    ]);
    expect(options.loading()).toBe(true);
    expect(trigger.getAttribute('aria-busy')).toBe('true');
    client.reply(page(['aaa'], false));
    fixture.detectChanges();
    expect(options.loading()).toBe(false);
    expect(optionLabels()).toEqual(['AAA']);

    const search = popup().querySelector<HTMLInputElement>('.search__input');
    if (search === null) {
      throw new Error('no search input');
    }
    search.value = 'acme';
    search.dispatchEvent(new Event('input'));
    fixture.detectChanges();
    expect(client.callsOf('keel.account.options')).toHaveLength(1);
    jest.advanceTimersByTime(DEBOUNCE - 1);
    expect(client.callsOf('keel.account.options')).toHaveLength(1);
    jest.advanceTimersByTime(1);
    fixture.detectChanges();
    const calls = client.callsOf('keel.account.options');
    expect(calls).toHaveLength(2);
    expect(calls[1].params).toEqual({ search: 'acme', scope: SCOPE, limit: OPTIONS_LIMIT });
    expect(options.loading()).toBe(true);

    client.reply(page(['acme-1', 'acme-2'], true));
    fixture.detectChanges();
    expect(options.loading()).toBe(false);
    expect(options.more()).toBe(true);
    expect(optionLabels()).toEqual(['ACME-1', 'ACME-2']);

    const more = popup().querySelector<HTMLButtonElement>('.more-bar button');
    expect(more?.textContent?.trim()).toBe('Load more');
    more?.click();
    fixture.detectChanges();
    expect(client.callsOf('keel.account.options')).toHaveLength(3);
    expect(client.callsOf('keel.account.options')[2].params).toEqual({
      search: 'acme',
      scope: SCOPE,
      limit: OPTIONS_LIMIT,
      cursor: 'acme-2',
    });
    client.reply(page(['acme-3'], false));
    fixture.detectChanges();
    expect(options.more()).toBe(false);
    expect(optionLabels()).toEqual(['ACME-1', 'ACME-2', 'ACME-3']);
    expect(popup().querySelector('.more-bar')).toBeNull();

    jest.advanceTimersByTime(DEBOUNCE);
    expect(client.callsOf('keel.account.options')).toHaveLength(3);
    fixture.destroy();
  });

  it('choosing an answered option writes its value and the trigger keeps the label', async () => {
    const client = new KeelMockClient({
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'keel.account.options': page(['acme-1'], false) },
    });
    const field = accountField(client);
    const fixture = await render(field, client);
    const element: HTMLElement = fixture.nativeElement;
    element.querySelector<HTMLButtonElement>('phx-select .trigger')?.click();
    fixture.detectChanges();
    popup().querySelector<HTMLElement>('[role=option]')?.click();
    fixture.detectChanges();
    expect(field.control.value).toBe('acme-1');
    expect(element.querySelector('phx-select .trigger')?.textContent?.trim()).toBe('ACME-1');
    fixture.destroy();
  });

  it('a form built without a client has no remote options and renders the select empty', async () => {
    const field = buildForm(EDIT, { params: SCOPE }).field('account');
    if (field === undefined) {
      throw new Error('no field account');
    }
    expect(field.options).toBeNull();
    const client = new KeelMockClient({ catalogue: { lang: 'en', messages: MESSAGES } });
    const fixture = await render(field, client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('phx-select')).not.toBeNull();
    expect(client.calls).toHaveLength(0);
    fixture.destroy();
  });

  it('optionsQuery omits an empty search, an empty scope and a missing cursor like the wire type', () => {
    expect(optionsQuery('', {})).toEqual({ limit: 20 });
    expect(optionsQuery('ac', { zone: 'z' }, 'c')).toEqual({
      search: 'ac',
      scope: { zone: 'z' },
      limit: 20,
      cursor: 'c',
    });
  });

  it('toSelectOption keeps hint and group and drops an icon phoenix does not know', () => {
    expect(toSelectOption({ value: 'a', label: 'A', hint: 'h', icon: 'user', group: 'g' })).toEqual(
      { value: 'a', label: 'A', hint: 'h', icon: 'user', group: 'g' },
    );
    expect(toSelectOption({ value: 'a', label: 'A', icon: 'no-such-icon', hint: null })).toEqual({
      value: 'a',
      label: 'A',
    });
  });

  it('replace drops a pending page, and a failed answer is kept on failure', () => {
    const client = new DeferredClient();
    const options = new RemoteOptions(client, 'keel.account.options', {});
    options.search('x');
    expect(options.loading()).toBe(true);
    options.replace([{ value: 'r', label: 'R' }]);
    expect(options.loading()).toBe(false);
    expect(options.choices()).toEqual([{ value: 'r', label: 'R' }]);
    expect(options.more()).toBe(false);

    options.loadMore();
    expect(client.calls).toHaveLength(1);

    options.search('y');
    client.reply({
      ok: false,
      error: { code: 'forbidden', key: 'keel.error.forbidden' },
    });
    expect(options.loading()).toBe(false);
    expect(options.failure()?.code).toBe('forbidden');
    expect(options.choices()).toEqual([{ value: 'r', label: 'R' }]);
    options.destroy();
  });
});
