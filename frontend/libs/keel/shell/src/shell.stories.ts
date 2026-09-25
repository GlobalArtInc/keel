import type { Routes } from '@angular/router';
import type { TaskRow } from '@globalart/keel/client';
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
import { expect, screen, userEvent, waitFor, within } from 'storybook/test';
import { KeelHomeComponent } from './home/home.component';
import { KeelLayoutComponent } from './layout/layout.component';
import { KeelLoginComponent } from './login/login.component';
import { KeelSetupComponent } from './login/setup.component';
import { screenRoutes } from './routes';

interface ShellArgs {
  screen: 'layout' | 'login' | 'setup';
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

const PROBLEM_KEY = 'note.overdue';
const PROBLEM = {
  id: '01J8A0000000000000000000P1',
  key: PROBLEM_KEY,
  subject: ACCOUNT_ID,
  severity: 'warning',
  params: { count: 1 },
  accountId: ACCOUNT_ID,
  source: 'poll',
  count: 1,
  open: true,
  raisedAt: '2026-09-12T08:00:00.000Z',
  updatedAt: '2026-09-12T08:00:00.000Z',
  clearedAt: null,
};

const MESSAGES_ROWS = [
  {
    id: '01J8A0000000000000000000M1',
    key: 'notify.problem.raised',
    params: PROBLEM,
    level: 'warning',
    source: 'problem',
    problem: PROBLEM.id,
    unread: true,
    readAt: null,
    createdAt: '2026-09-12T08:00:00.000Z',
  },
];

function job(id: string, state: TaskRow['state'], done: number, total: number): TaskRow {
  return {
    id,
    function: 'note.export',
    class: 'export',
    state,
    progress: `${done}/${total}`,
    done,
    total,
    resumable: true,
    requestId: `req-${id.slice(-2)}`,
    createdAt: '2026-09-12T09:00:00.000Z',
    accountId: ACCOUNT_ID,
    principalId: '01J8A0000000000000000000U1',
  };
}

const JOBS = [
  job('01J8A0000000000000000000J1', 'running', 2, 4),
  job('01J8A0000000000000000000J2', 'done', 3, 3),
];

const CALLS: Record<string, CannedAnswer> = {
  'note.list': pageEnvelope(NOTES),
  'keel.account.list': pageEnvelope(ACCOUNT_ROWS),
  'keel.account.options': okEnvelope(ACCOUNTS),
  'task.list': pageEnvelope(JOBS),
  'task.get': okEnvelope({ ...JOBS[0], params: {}, logTail: ['exported Alpha', 'exported Beta'] }),
  'notify.message.list': pageEnvelope(MESSAGES_ROWS),
  'notify.problem.list': pageEnvelope([PROBLEM]),
  'keel.widget.accounts': okEnvelope({ value: 42, unit: 'count', series: 'keel.accounts' }),
  'dns.widget.zones': okEnvelope({ value: 7, unit: 'count' }),
  'stats.series': okEnvelope({
    key: 'keel.accounts',
    unit: 'count',
    resolution: 'hour',
    points: [38, 40, 42].map((value, day) => ({
      at: `2026-09-2${day}T00:00:00Z`,
      value,
      min: value,
      max: value,
      sum: value,
      count: 1,
    })),
  }),
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

function demoOnly(boot: Boot): Boot {
  const others = new Set(
    boot.products.filter((product) => product.id !== 'demo').map((product) => product.id),
  );
  return {
    ...boot,
    products: boot.products.filter((product) => !others.has(product.id)),
    brand: KEEL_BOOT.one.brand,
    features: boot.features.filter((feature) => !others.has(feature.split('.')[0] ?? '')),
    menu: boot.menu.filter((group) => !others.has(group.product ?? '')),
  };
}

function actingBoot(): Boot {
  const alice = KEEL_PRINCIPALS.user;
  const root = KEEL_PRINCIPALS.admin;
  if (alice === null || root === null) {
    throw new Error('the user and admin fixtures are principals');
  }
  return { ...demoOnly(keelDump('user').boot), principal: actingAs(alice, root) };
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
  decorators: [
    moduleMetadata({ imports: [KeelLayoutComponent, KeelLoginComponent, KeelSetupComponent] }),
  ],
  parameters: { layout: 'fullscreen' },
  argTypes: { screen: { control: false } },
  args: { screen: 'layout' },
  render: (args) => ({
    props: args,
    template: `
      @if (screen === 'login') {
        <keel-login />
      } @else if (screen === 'setup') {
        <keel-setup key="kok_story" />
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
      boot: demoOnly(keelDump('guest').boot),
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

export const Setup: Story = {
  args: { screen: 'setup' },
  decorators: [
    withKeel({
      level: 'guest',
      calls: {
        'keel.setup.admin': errorEnvelope({
          code: 'validation',
          key: 'keel.error.validation',
          fields: [{ path: 'password', key: 'auth.error.password_short', params: { min: 12 } }],
        }),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByRole('heading', { name: 'Create the first administrator' }),
    ).toBeInTheDocument();
    await userEvent.type(canvas.getByLabelText(/^Login/), 'first');
    await userEvent.type(canvas.getByLabelText(/^Password/), 'short');
    await userEvent.click(canvas.getByRole('button', { name: 'Create the administrator' }));
    await expect(await canvas.findByRole('alert')).toHaveTextContent(
      'The password must be at least 12 characters long',
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

export const Tasks: Story = {
  decorators: [withKeel({ boot: KEEL_BOOT.one, calls: CALLS, routes: routesTo('note.list') })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    const item = canvasElement.querySelector<HTMLElement>('phx-rail-item[data-rail="tasks"]');
    await waitFor(() => expect(item?.querySelector('.badge')).toHaveTextContent('1'));
    await userEvent.click(canvas.getByRole('button', { name: 'Tasks' }));
    const drawer = await screen.findByRole('heading', { name: 'Tasks' });
    await expect(drawer).toBeInTheDocument();
    const jobs = await screen.findAllByRole('button', { name: /Export notes/ });
    await expect(jobs).toHaveLength(JOBS.length);
    await userEvent.click(jobs[0]);
    await expect(await screen.findByRole('textbox', { name: 'Log' })).toHaveValue(
      'exported Alpha\nexported Beta',
    );
    await expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
  },
};

export const Notifications: Story = {
  decorators: [withKeel({ boot: KEEL_BOOT.one, calls: CALLS, routes: routesTo('note.list') })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    const item = canvasElement.querySelector<HTMLElement>(
      'phx-rail-item[data-rail="notifications"]',
    );
    await waitFor(() => expect(item?.querySelector('.badge')).toHaveTextContent('1'));
    await expect(
      canvasElement.querySelector('phx-sidebar-item[data-func="note.list"] .badge'),
    ).toHaveTextContent('1');
    await userEvent.click(canvas.getByRole('button', { name: 'Notifications' }));
    await expect(
      await screen.findByText(`Problem ${PROBLEM_KEY} raised for ${ACCOUNT_ID}`),
    ).toBeInTheDocument();
    await expect(screen.getByRole('heading', { name: 'Open problems' })).toBeInTheDocument();
    await expect(
      document.querySelector(`[data-problems] [data-problem="${PROBLEM_KEY}"]`),
    ).toHaveTextContent('Seen 1 times');
  },
};

export const Home: Story = {
  decorators: [
    withKeel({
      boot: KEEL_BOOT.one,
      calls: CALLS,
      routes: [
        { path: SCREEN_PATH, children: screenRoutes([]) },
        { path: '', pathMatch: 'full', component: KeelHomeComponent },
      ],
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole('heading', { name: 'Overview' })).toBeInTheDocument();
    const card = canvasElement.querySelector(
      'phx-widget-card[data-widget-func="notify.problem.list"]',
    );
    await expect(card).toHaveTextContent('Problems');
    await expect(await canvas.findByText(PROBLEM_KEY)).toBeInTheDocument();
    await expect(canvas.getByRole('link', { name: 'Open' })).toHaveAttribute(
      'href',
      `/${SCREEN_PATH}/notify.problem.list`,
    );
    const accounts = canvasElement.querySelector(
      'phx-widget-card[data-widget-func="keel.widget.accounts"]',
    );
    await waitFor(() =>
      expect(accounts?.querySelector('keel-sparkline')).toHaveAttribute('data-points', '3'),
    );
    await expect(accounts?.querySelector('phx-stat')).toHaveTextContent('42');
  },
};

export const About: Story = {
  decorators: [withKeel({ boot: KEEL_BOOT.one, calls: CALLS, routes: routesTo('note.list') })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    await userEvent.click(canvas.getByRole('button', { name: 'Help' }));
    await expect(
      await screen.findByRole('heading', { name: 'About this installation' }),
    ).toBeInTheDocument();
    const about = document.querySelector('keel-about');
    await expect(about?.querySelector('phx-prop-list')).toHaveTextContent(
      `Version ${KEEL_BOOT.one.keel.version} API version ${KEEL_BOOT.one.keel.api}`,
    );
    await expect(about?.querySelector('.product[data-product="demo"]')).toHaveTextContent(
      'Keel Demo',
    );
  },
};
