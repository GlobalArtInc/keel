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
