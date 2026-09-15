import type { Routes } from '@angular/router';
import type { Boot, Choices } from '@globalart/keel/meta';
import { SCREEN_PATH } from '@globalart/keel/renderer';
import {
  ACCOUNT_ID,
  KEEL_BOOT,
  KEEL_PRINCIPALS,
  actingAs,
  errorEnvelope,
  keelDump,
  okEnvelope,
  pageEnvelope,
  withKeel,
  type CannedAnswer,
} from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, userEvent, within } from 'storybook/test';
import { KeelLayoutComponent } from './layout/layout.component';
import { KeelLoginComponent } from './login/login.component';
import { screenRoutes } from './routes';

interface ShellArgs {
  screen: 'layout' | 'login';
}

const ACCOUNTS: Choices = {
  data: [
    { value: ACCOUNT_ID, label: 'Root account' },
    { value: '01J8A0000000000000000000A2', label: 'Acme Ltd' },
  ],
  more: false,
};

const NOTES = [
  {
    id: '01J8A0000000000000000000N1',
    accountId: ACCOUNT_ID,
    title: 'Renew the certificates',
    body: 'Every host of the cluster, before the end of the month.',
    kind: 'todo',
    tags: ['ops', 'tls'],
    due: '2026-09-30',
    createdAt: '2026-09-01T09:30:00.000Z',
    updatedAt: '2026-09-12T16:30:00.000Z',
  },
  {
    id: '01J8A0000000000000000000N2',
    accountId: ACCOUNT_ID,
    title: 'Release notes 0.1',
    body: 'Draft the notes for the first release.',
    kind: 'memo',
    tags: ['docs'],
    due: null,
    createdAt: '2026-09-03T11:30:00.000Z',
    updatedAt: '2026-09-10T08:30:00.000Z',
  },
];

const ACCOUNT_ROWS = [
  {
    id: ACCOUNT_ID,
    kind: 'root',
    parentId: null,
    path: ACCOUNT_ID,
    state: 'active',
    externalId: null,
    origin: 'install',
    createdAt: '2026-09-01T08:30:00.000Z',
    updatedAt: '2026-09-01T08:30:00.000Z',
  },
  {
    id: '01J8A0000000000000000000A2',
    kind: 'customer',
    parentId: ACCOUNT_ID,
    path: `${ACCOUNT_ID}/01J8A0000000000000000000A2`,
    state: 'active',
    externalId: 'crm-4711',
    origin: 'api',
    createdAt: '2026-09-05T10:30:00.000Z',
    updatedAt: '2026-09-11T17:30:00.000Z',
  },
];

const CALLS: Record<string, CannedAnswer> = {
  'note.list': pageEnvelope(NOTES),
  'keel.account.list': pageEnvelope(ACCOUNT_ROWS),
  'keel.account.options': okEnvelope(ACCOUNTS),
};

const FIXTURE_MESSAGES = {
  menu: { dns: 'DNS', hosting: 'Hosting' },
  product: { dns: { brand: 'KeelDNS' }, panel: { brand: 'KeelPanel' } },
  dns: { zone: { list: { title: 'Zones' } } },
  hosting: { site: { list: { title: 'Sites' } } },
};

function routesTo(home: string): Routes {
  return [
    { path: SCREEN_PATH, children: screenRoutes([]) },
    { path: '', pathMatch: 'full', redirectTo: `${SCREEN_PATH}/${home}` },
  ];
}

function speaking(boot: Boot, lang: string): Boot {
  return boot.principal ? { ...boot, principal: { ...boot.principal, lang } } : boot;
}

function actingBoot(): Boot {
  const alice = KEEL_PRINCIPALS.user;
  const root = KEEL_PRINCIPALS.admin;
  if (alice === null || root === null) {
    throw new Error('the user and admin fixtures are principals');
  }
  return { ...keelDump('user').boot, principal: actingAs(alice, root) };
}

interface ShownSection {
  product: string | null;
  heading: string | null;
  groups: string[];
}

function brandTextOf(root: HTMLElement): string | null {
  return root.querySelector('phx-brand-logo .brand')?.textContent?.trim() ?? null;
}

function sectionsOf(root: HTMLElement): ShownSection[] {
  return [...root.querySelectorAll<HTMLElement>('phx-sidebar-group.section')].map((section) => ({
    product: section.dataset['product'] ?? null,
    heading: section.querySelector(':scope > .eyebrow')?.textContent?.trim() ?? null,
    groups: [...section.querySelectorAll<HTMLElement>('phx-sidebar-group.group')].map(
      (group) => group.dataset['group'] ?? '',
    ),
  }));
}

const meta: Meta<ShellArgs> = {
  title: 'Shell',
  decorators: [moduleMetadata({ imports: [KeelLayoutComponent, KeelLoginComponent] })],
  parameters: { layout: 'fullscreen' },
  argTypes: { screen: { control: false } },
  args: { screen: 'layout' },
  render: (args) => ({
    props: args,
    template: `
      @if (screen === 'login') {
        <keel-login />
      } @else {
        <keel-layout />
      }
    `,
  }),
};

export default meta;
type Story = StoryObj<ShellArgs>;

export const NoProduct: Story = {
  decorators: [
    withKeel({ boot: KEEL_BOOT.none, calls: CALLS, routes: routesTo('keel.account.list') }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('customer');
    await expect(brandTextOf(canvasElement)).toBe('Keel');
    await expect(canvasElement.querySelector('phx-brand-logo img')).toBeNull();
    await expect(sectionsOf(canvasElement)).toEqual([
      { product: null, heading: null, groups: ['system'] },
    ]);
    await expect(canvas.getByRole('heading', { name: 'Accounts' })).toBeInTheDocument();
  },
};

export const OneProduct: Story = {
  decorators: [withKeel({ boot: KEEL_BOOT.one, calls: CALLS, routes: routesTo('note.list') })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    const logo = canvasElement.querySelector('phx-brand-logo img');
    await expect(logo).toHaveAttribute('alt', 'Keel Demo');
    await expect(logo).toHaveAttribute('src', '/brand/logo.svg');
    await expect(sectionsOf(canvasElement)).toEqual([
      { product: 'demo', heading: null, groups: ['demo'] },
      { product: null, heading: null, groups: ['system'] },
    ]);
    await expect(canvas.getByRole('link', { name: /Notes/ })).toBeInTheDocument();
    await expect(canvas.getByRole('heading', { name: 'Notes' })).toBeInTheDocument();
  },
};

export const TwoProducts: Story = {
  decorators: [
    withKeel({
      boot: speaking(KEEL_BOOT.two, 'en'),
      calls: CALLS,
      messages: FIXTURE_MESSAGES,
      routes: routesTo('keel.account.list'),
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('customer');
    await expect(brandTextOf(canvasElement)).toBe('Keel');
    await expect(canvasElement.querySelector('phx-brand-logo img')).toBeNull();
    await expect(sectionsOf(canvasElement)).toEqual([
      { product: 'dns', heading: 'KeelDNS', groups: ['dns'] },
      { product: 'panel', heading: 'KeelPanel', groups: ['hosting'] },
      { product: null, heading: null, groups: ['system'] },
    ]);
    await expect(canvas.getByRole('link', { name: /Zones/ })).toBeInTheDocument();
    await expect(canvas.getByRole('link', { name: /Sites/ })).toBeInTheDocument();
  },
};

export const ActingAs: Story = {
  name: 'Acting as',
  decorators: [
    withKeel({ level: 'user', boot: actingBoot(), calls: CALLS, routes: routesTo('note.list') }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    await expect(canvasElement.querySelector('[data-acting-as]')).toHaveTextContent(
      'You are acting as alice',
    );
    await expect(
      canvas.getByRole('button', { name: 'Return to your own session' }),
    ).toBeInTheDocument();
    await expect(sectionsOf(canvasElement)).toEqual([
      { product: 'demo', heading: null, groups: ['demo'] },
      { product: null, heading: null, groups: ['system'] },
    ]);
  },
};

export const Login: Story = {
  args: { screen: 'login' },
  decorators: [
    withKeel({
      level: 'guest',
      calls: {
        'auth.login': errorEnvelope({ code: 'unauthenticated', key: 'auth.error.credentials' }),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('Keel Demo')).toBeInTheDocument();
    await userEvent.type(canvas.getByLabelText(/^Login/), 'alice');
    await userEvent.type(canvas.getByLabelText(/^Password/), 'wrong');
    await userEvent.click(canvas.getByRole('button', { name: 'Sign in' }));
    await expect(await canvas.findByRole('alert')).toHaveTextContent(
      'The login or the password is wrong',
    );
  },
};

export const Totp: Story = {
  args: { screen: 'login' },
  decorators: [
    withKeel({
      level: 'guest',
      calls: {
        'auth.login': okEnvelope({ state: 'totp', challenge: 'ch_01J9' }),
        'auth.totp.verify': errorEnvelope({ code: 'unauthenticated', key: 'auth.error.code' }),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(await canvas.findByLabelText(/^Login/), 'root');
    await userEvent.type(canvas.getByLabelText(/^Password/), 'correct horse battery staple');
    await userEvent.click(canvas.getByRole('button', { name: 'Sign in' }));
    await expect(await canvas.findByLabelText(/^Code/)).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Verify' })).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Back' })).toBeInTheDocument();
  },
};
