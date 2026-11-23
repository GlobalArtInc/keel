import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelClient, KeelI18n } from '@globalart/keel/client';
import { KeelFormComponent, buildForm, type KeelForm } from '@globalart/keel/forms';
import type { Descriptor } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');

const MESSAGES = {
  dns: {
    zone: {
      edit: { title: 'Zone' },
      name: 'Zone name',
      kind: 'Kind',
      ttl: 'TTL',
      account: 'Account',
      master_ips: 'Master servers',
      tsig_key: 'TSIG key',
      page: { transfer: 'Transfer' },
    },
  },
  keel: { page: { main: 'Main' } },
};

async function render(form: KeelForm): Promise<ComponentFixture<KeelFormComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelFormComponent],
    providers: [
      providePhoenixIcons(),
      {
        provide: KeelClient,
        useValue: new KeelMockClient({ catalogue: { lang: 'en', messages: MESSAGES } }),
      },
    ],
  });
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelFormComponent);
  fixture.componentRef.setInput('form', form);
  fixture.detectChanges();
  return fixture;
}

function fieldNames(fixture: ComponentFixture<KeelFormComponent>): string[] {
  const element: HTMLElement = fixture.nativeElement;
  return [...element.querySelectorAll('keel-field .label__text')].map(
    (label) => label.textContent ?? '',
  );
}

describe('keel-form', () => {
  it('a create with kind master is one phx-form-section and turns into tabs once kind is slave', async () => {
    const form = buildForm(EDIT, { values: { kind: 'master' } });
    const fixture = await render(form);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('phx-form-sheet h1')?.textContent).toBe('Zone');
    expect(element.getAttribute('data-form')).toBe('dns.zone.edit');
    expect(element.getAttribute('data-mode')).toBe('create');
    expect(element.querySelectorAll('phx-form-section')).toHaveLength(1);
    expect(element.querySelector('phx-form-section .title')?.textContent?.trim()).toBe('Main');
    expect(element.querySelector('phx-tab-group')).toBeNull();
    expect(fieldNames(fixture)).toEqual(['Zone name', 'Kind', 'TTL', 'Account']);

    form.group.controls['kind'].setValue('slave');
    fixture.detectChanges();
    expect(element.querySelector('phx-form-section')).toBeNull();
    const tabs = [...element.querySelectorAll('phx-tab-group [role=tab]')];
    expect(tabs.map((tab) => tab.textContent?.trim())).toEqual(['Main', 'Transfer']);
    expect(fieldNames(fixture)).toEqual([
      'Zone name',
      'Kind',
      'TTL',
      'Account',
      'Master servers',
      'TSIG key',
    ]);
    expect(element.querySelector('keel-page[data-page=transfer] phx-tags-field')).not.toBeNull();
    expect(element.querySelector('keel-page[data-page=transfer] phx-password')).not.toBeNull();

    form.group.controls['kind'].setValue('master');
    fixture.detectChanges();
    expect(element.querySelector('phx-tab-group')).toBeNull();
    expect(element.querySelectorAll('phx-form-section')).toHaveLength(1);
  });

  it('an edit renders the zone name disabled with its value kept', async () => {
    const form = buildForm(EDIT, {
      params: { id: '01J8A0000000000000000000Z1' },
      values: { name: 'a.org', kind: 'master', ttl: 300 },
    });
    const fixture = await render(form);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-mode')).toBe('edit');
    const name = element.querySelector<HTMLInputElement>('phx-input input');
    expect(name?.value).toBe('a.org');
    expect(name?.disabled).toBe(true);
    expect(form.group.getRawValue()['name']).toBe('a.org');
  });

  it('the sheet closes through the closed output', async () => {
    const form = buildForm(EDIT);
    const fixture = await render(form);
    const element: HTMLElement = fixture.nativeElement;
    const closed = jest.fn();
    fixture.componentInstance.closed.subscribe(closed);
    const close = element.querySelector<HTMLButtonElement>('phx-form-sheet .header__close');
    expect(close).not.toBeNull();
    close?.click();
    await fixture.whenStable();
    expect(closed).toHaveBeenCalledTimes(1);

    fixture.componentRef.setInput('closable', false);
    fixture.detectChanges();
    expect(element.querySelector('phx-form-sheet .header__close')).toBeNull();
  });

  it('a form that declares one page draws its fields without a section', async () => {
    const single = JSON.parse(JSON.stringify(EDIT)) as Descriptor;
    single.ui?.pages?.splice(1);
    const fixture = await render(buildForm(single, { values: { kind: 'master' } }));
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('phx-form-section')).toBeNull();
    expect(element.querySelectorAll('keel-page')).toHaveLength(1);
    expect(fieldNames(fixture)).toEqual(['Zone name', 'Kind', 'TTL', 'Account']);
  });
});
