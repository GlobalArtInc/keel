import { ACCOUNT_ID, errorEnvelope, okEnvelope, withKeel } from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, userEvent, within } from 'storybook/test';
import { KeelScreenComponent } from '../screen';

interface ObjectArgs {
  func: string;
  params: Record<string, unknown> | null;
}

const CALL_ID = '01J8A0000000000000000000C1';

const CALL = {
  id: CALL_ID,
  at: '2026-09-14T09:30:00.000Z',
  requestId: 'req-7f3a2c1b',
  function: 'note.edit',
  module: 'note',
  principalId: '01J8A0000000000000000000B1',
  principalLogin: 'root',
  accountId: ACCOUNT_ID,
  actorPrincipalId: null,
  actorLogin: null,
  via: 'http',
  ip: '203.0.113.7',
  paramsRedacted: { id: '01J8A0000000000000000000N1', title: 'Renew the certificates' },
  subject: '01J8A0000000000000000000N1',
  after: { title: 'Renew the certificates', kind: 'todo' },
  status: 'committed',
  errorCode: null,
  errorKey: null,
  elapsedMs: 18,
};

const meta: Meta<ObjectArgs> = {
  title: 'Renderer/Object',
  decorators: [moduleMetadata({ imports: [KeelScreenComponent] })],
  parameters: { layout: 'padded' },
  argTypes: {
    func: { control: false },
    params: { control: 'object' },
  },
  args: { func: 'audit.get', params: { id: CALL_ID } },
  render: (args) => ({
    props: args,
    template: '<keel-screen [func]="func" [params]="params" />',
  }),
};

export default meta;
type Story = StoryObj<ObjectArgs>;

export const AuditCall: Story = {
  name: 'Audit call',
  decorators: [withKeel({ calls: { 'audit.get': okEnvelope(CALL) } })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText('req-7f3a2c1b')).toBeInTheDocument();
    await expect(canvas.getByText('note.edit')).toBeInTheDocument();
    await expect(canvas.getByText('203.0.113.7')).toBeInTheDocument();
    await expect(canvas.getByText('Request')).toBeInTheDocument();
  },
};

export const Forbidden: Story = {
  decorators: [
    withKeel({
      calls: {
        'audit.get': [
          errorEnvelope({ code: 'forbidden', key: 'keel.error.forbidden' }, 'req-refused'),
          okEnvelope(CALL),
        ],
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/request req-refused/)).toBeInTheDocument();
    await userEvent.click(canvas.getByRole('button', { name: 'Retry' }));
    await expect(await canvas.findByText('req-7f3a2c1b')).toBeInTheDocument();
  },
};

const STATUS = {
  running: '0.1.0',
  runningContract: 1,
  channel: 'stable',
  channelUrl: 'https://dl.globalart.hu/keel/',
  available: '0.2.0',
  notes: 'Second release: the update module, the release channel and the privileged unit.',
  minUpgradeFrom: '0.1.0',
  contract: 1,
  upToDate: false,
  allowed: true,
  blockedBy: null,
  previous: null,
};

export const UpdateAvailable: Story = {
  name: 'Updates with a version available',
  args: { func: 'update.check', params: null },
  decorators: [withKeel({ calls: { 'update.check': okEnvelope(STATUS) } })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole('heading', { name: 'Updates' })).toBeInTheDocument();
    await expect(canvas.getByText('0.2.0')).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Update' })).toBeInTheDocument();
    await expect(canvas.queryByRole('button', { name: 'Roll back' })).not.toBeInTheDocument();
  },
};

export const UpdateBlocked: Story = {
  name: 'Updates blocked by min_upgrade_from',
  args: { func: 'update.check', params: null },
  decorators: [
    withKeel({
      calls: {
        'update.check': okEnvelope({
          ...STATUS,
          available: '0.3.0',
          minUpgradeFrom: '0.2.0',
          allowed: false,
          blockedBy: 'min_upgrade_from',
          previous: '0.0.9',
        }),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole('heading', { name: 'Updates' })).toBeInTheDocument();
    await expect(canvas.queryByRole('button', { name: 'Update' })).not.toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Roll back' })).toBeInTheDocument();
    await expect(
      canvas.getByText('The running version is below the minimum the channel upgrades from'),
    ).toBeInTheDocument();
  },
};
