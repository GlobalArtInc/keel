import type { Choices } from '@globalart/keel/meta';
import { ACCOUNT_ID, okEnvelope, pageEnvelope, withKeel } from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, userEvent, within } from 'storybook/test';
import { KeelScreenComponent } from '../screen';

interface ListArgs {
  func: string;
  params: Record<string, unknown> | null;
}

interface Note {
  id: string;
  accountId: string;
  title: string;
  body: string;
  kind: 'memo' | 'todo';
  tags: string[];
  due: string | null;
  createdAt: string;
  updatedAt: string;
}

interface Account {
  id: string;
  kind: string;
  parentId: string | null;
  path: string;
  state: 'active' | 'suspended' | 'deleting';
  externalId: string | null;
  origin: string | null;
  createdAt: string;
  updatedAt: string;
}

interface SettingRow {
  key: string;
  module: string;
  schema: unknown;
  default: unknown;
  level: 'user' | 'reseller' | 'admin';
  value: unknown;
  scope: string | null;
}

interface AuditCall {
  id: string;
  at: string;
  requestId: string;
  function: string;
  module: string;
  principalId: string | null;
  principalLogin: string;
  accountId: string;
  actorPrincipalId: string | null;
  actorLogin: string | null;
  via: string;
  ip: string | null;
  paramsRedacted: unknown;
  subject: string | null;
  after: unknown;
  status: 'committed' | 'failed';
  errorCode: string | null;
  errorKey: string | null;
  elapsedMs: number;
}

function id(suffix: string): string {
  return `01J8A${'0'.repeat(21 - suffix.length)}${suffix}`;
}

function at(day: number, hour: number): string {
  return new Date(Date.UTC(2026, 8, day, hour, 30)).toISOString();
}

const ACCOUNTS: Choices = {
  data: [
    { value: ACCOUNT_ID, label: 'Root account' },
    { value: id('A2'), label: 'Acme Ltd' },
  ],
  more: false,
};

const NOTES: Note[] = [
  {
    id: id('N1'),
    accountId: ACCOUNT_ID,
    title: 'Renew the certificates',
    body: 'Every host of the cluster, before the end of the month.',
    kind: 'todo',
    tags: ['ops', 'tls'],
    due: '2026-09-30',
    createdAt: at(1, 9),
    updatedAt: at(12, 16),
  },
  {
    id: id('N2'),
    accountId: ACCOUNT_ID,
    title: 'Release notes 0.1',
    body: 'Draft the notes for the first release.',
    kind: 'memo',
    tags: ['docs'],
    due: null,
    createdAt: at(3, 11),
    updatedAt: at(10, 8),
  },
  {
    id: id('N3'),
    accountId: id('A2'),
    title: 'Call the reseller back',
    body: '',
    kind: 'todo',
    tags: [],
    due: '2026-09-18',
    createdAt: at(8, 14),
    updatedAt: at(8, 14),
  },
];

const ACCOUNT_ROWS: Account[] = [
  {
    id: ACCOUNT_ID,
    kind: 'root',
    parentId: null,
    path: ACCOUNT_ID,
    state: 'active',
    externalId: null,
    origin: 'install',
    createdAt: at(1, 8),
    updatedAt: at(1, 8),
  },
  {
    id: id('A2'),
    kind: 'customer',
    parentId: ACCOUNT_ID,
    path: `${ACCOUNT_ID}/${id('A2')}`,
    state: 'suspended',
    externalId: 'crm-4711',
    origin: 'api',
    createdAt: at(5, 10),
    updatedAt: at(11, 17),
  },
];

const SETTINGS: SettingRow[] = [
  {
    key: 'auth.password.min_length',
    module: 'auth',
    schema: { type: 'integer', minimum: 8 },
    default: 12,
    level: 'admin',
    value: 12,
    scope: null,
  },
  {
    key: 'auth.session.idle_minutes',
    module: 'auth',
    schema: { type: 'integer', minimum: 5 },
    default: 30,
    level: 'admin',
    value: 45,
    scope: null,
  },
  {
    key: 'note.limit',
    module: 'note',
    schema: { type: 'integer', minimum: 1 },
    default: 1000,
    level: 'reseller',
    value: 1000,
    scope: id('A2'),
  },
];

const CALLS: AuditCall[] = [
  {
    id: id('C1'),
    at: at(14, 9),
    requestId: 'req-7f3a2c1b',
    function: 'note.edit',
    module: 'note',
    principalId: id('B1'),
    principalLogin: 'root',
    accountId: ACCOUNT_ID,
    actorPrincipalId: null,
    actorLogin: null,
    via: 'http',
    ip: '203.0.113.7',
    paramsRedacted: { title: 'Renew the certificates' },
    subject: id('N1'),
    after: null,
    status: 'committed',
    errorCode: null,
    errorKey: null,
    elapsedMs: 18,
  },
  {
    id: id('C2'),
    at: at(14, 10),
    requestId: 'req-9d4e0a62',
    function: 'note.delete',
    module: 'note',
    principalId: id('B2'),
    principalLogin: 'alice',
    accountId: id('A2'),
    actorPrincipalId: id('B1'),
    actorLogin: 'root',
    via: 'http',
    ip: '203.0.113.9',
    paramsRedacted: { ids: [id('N3')] },
    subject: id('N3'),
    after: null,
    status: 'failed',
    errorCode: 'forbidden',
    errorKey: 'note.error.locked',
    elapsedMs: 4,
  },
];

const meta: Meta<ListArgs> = {
  title: 'Renderer/List',
  decorators: [moduleMetadata({ imports: [KeelScreenComponent] })],
  parameters: { layout: 'padded' },
  argTypes: {
    func: { control: false },
    params: { control: 'object' },
  },
  args: { func: 'note.list', params: null },
  render: (args) => ({
    props: args,
    template: '<keel-screen [func]="func" [params]="params" />',
  }),
};

export default meta;
type Story = StoryObj<ListArgs>;

export const Notes: Story = {
  decorators: [
    withKeel({
      calls: {
        'note.list': pageEnvelope(NOTES),
        'keel.account.options': okEnvelope(ACCOUNTS),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('Renew the certificates')).toBeInTheDocument();
    await expect(canvas.getByRole('heading', { name: 'Notes' })).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: /New note/ })).toBeInTheDocument();
    await expect(
      canvas.getAllByRole('columnheader').map((cell) => cell.textContent?.trim()),
    ).toEqual(expect.arrayContaining(['Title', 'Kind', 'Tags', 'Due', 'Updated', 'Account']));
    await expect(canvas.getAllByText('To do')).toHaveLength(2);
  },
};

export const NotesAsUser: Story = {
  name: 'Notes as a user',
  decorators: [withKeel({ level: 'user', calls: { 'note.list': pageEnvelope(NOTES) } })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('Release notes 0.1')).toBeInTheDocument();
    await expect(canvas.queryByRole('columnheader', { name: /Account/ })).not.toBeInTheDocument();
  },
};

export const Selection: Story = {
  decorators: [
    withKeel({
      calls: {
        'note.list': pageEnvelope(NOTES),
        'keel.account.options': okEnvelope(ACCOUNTS),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    const boxes = canvas.getAllByRole('checkbox', { name: /Select row/ });
    await userEvent.click(boxes[0]);
    await userEvent.click(boxes[2]);
    await expect(await canvas.findByRole('button', { name: /Delete/ })).toBeInTheDocument();
  },
};

export const Empty: Story = {
  parameters: { a11y: { config: { rules: [{ id: 'heading-order', enabled: false }] } } },
  decorators: [
    withKeel({
      calls: {
        'note.list': pageEnvelope([]),
        'keel.account.options': okEnvelope(ACCOUNTS),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('No notes yet')).toBeInTheDocument();
  },
};

export const Accounts: Story = {
  args: { func: 'keel.account.list' },
  decorators: [
    withKeel({
      calls: {
        'keel.account.list': pageEnvelope(ACCOUNT_ROWS),
        'keel.account.options': okEnvelope(ACCOUNTS),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('customer')).toBeInTheDocument();
    await expect(canvas.getByRole('heading', { name: 'Accounts' })).toBeInTheDocument();
    await expect(canvas.getByText('Suspended')).toBeInTheDocument();
  },
};

export const Settings: Story = {
  args: { func: 'settings.list' },
  decorators: [withKeel({ calls: { 'settings.list': pageEnvelope(SETTINGS) } })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('auth.password.min_length')).toBeInTheDocument();
    await expect(canvas.getByRole('heading', { name: 'Settings' })).toBeInTheDocument();
  },
};

export const Audit: Story = {
  args: { func: 'audit.list' },
  decorators: [withKeel({ calls: { 'audit.list': pageEnvelope(CALLS) } })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('note.delete')).toBeInTheDocument();
    await expect(canvas.getByRole('heading', { name: 'Audit trail' })).toBeInTheDocument();
    await expect(canvas.getByText('Failed')).toBeInTheDocument();
  },
};
