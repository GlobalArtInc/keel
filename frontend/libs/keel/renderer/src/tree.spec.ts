import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const DUMP = resolve(__dirname, '../../../../../contracts/dump');
const LIST = 'keel.account.list';
const ROOT = '01J8A0000000000000000000R0';
const RESELLER = '01J8A0000000000000000000R1';
const CUSTOMER = '01J8A0000000000000000000R2';

const ACCOUNTS = JSON.parse(
  readFileSync(join(DUMP, 'admin/functions', `${LIST}.json`), 'utf8'),
) as Descriptor;
const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;

const MESSAGES = {
  keel: {
    account: {
      list: { title: 'Accounts' },
      action: {
        create: 'Create',
        edit: 'Edit',
        suspend: 'Suspend',
        resume: 'Resume',
        delete: 'Delete',
      },
    },
  },
};

function page(rows: { id: string; parentId?: string; state: string }[]): Envelope {
  return {
    ok: true,
    data: { data: rows.map((row) => ({ kind: 'user', ...row })), total: rows.length, totals: null },
    meta: { requestId: 'req-tree' },
  };
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  fixture.detectChanges();
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

function labels(element: HTMLElement): string[] {
  return [...element.querySelectorAll('phx-tree-node .text')].map(
    (node) => node.textContent?.trim() ?? '',
  );
}

describe('keel-tree', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('a_tree_loads_children_on_expand', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [ACCOUNTS],
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        [LIST]: [
          page([{ id: ROOT, state: 'active' }]),
          page([
            { id: RESELLER, parentId: ROOT, state: 'active' },
            { id: CUSTOMER, parentId: ROOT, state: 'suspended' },
          ]),
        ],
      },
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
    await harness.navigateByUrl(`/f/${LIST}`, KeelScreenComponent);
    await settle(harness.fixture);
    const element = harness.routeNativeElement;
    if (element === null) {
      throw new Error('the screen did not render');
    }

    const tree = element.querySelector(`keel-tree[data-tree="${LIST}"]`);
    expect(tree?.getAttribute('data-phase')).toBe('loaded');
    expect(element.querySelector('keel-list')).toBeNull();
    expect(labels(element)).toEqual([ROOT]);
    expect(client.callsOf(LIST).map((call) => call.params)).toEqual([
      { page: 1, pageSize: 200, scope: { parentId: null } },
    ]);

    element.querySelector<HTMLButtonElement>('phx-tree-node .chevron')?.click();
    await settle(harness.fixture);
    expect(client.callsOf(LIST).map((call) => call.params)).toEqual([
      { page: 1, pageSize: 200, scope: { parentId: null } },
      { page: 1, pageSize: 200, scope: { parentId: ROOT } },
    ]);
    expect(labels(element)).toEqual([ROOT, RESELLER, CUSTOMER]);

    const nodes = element.querySelectorAll<HTMLElement>('cdk-tree-node');
    nodes[2]?.click();
    await settle(harness.fixture);
    const bar = element.querySelector('phx-selection-bar');
    expect(bar?.querySelector('.count')?.textContent?.trim()).toBe(CUSTOMER);
    expect(
      [...(bar?.querySelectorAll('.actions button') ?? [])].map((button) =>
        button.textContent?.trim(),
      ),
    ).toEqual(['Edit', 'Resume', 'Delete']);
  });
});
