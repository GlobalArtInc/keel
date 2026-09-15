import type { Choices } from '@globalart/keel/meta';
import {
  ACCOUNT_ID,
  errorEnvelope,
  okEnvelope,
  withKeel,
  type CannedAnswer,
} from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, screen, userEvent, waitFor, within } from 'storybook/test';
import { KeelScreenComponent } from '../screen';

interface FormArgs {
  func: string;
  params: Record<string, unknown> | null;
}

const NOTE_ID = '01J8A0000000000000000000N1';
const DUE = '2026-09-22';

const NOTE = {
  id: NOTE_ID,
  accountId: ACCOUNT_ID,
  title: 'Renew the certificates',
  body: 'Every host of the cluster, before the end of the month.',
  kind: 'todo',
  tags: ['ops', 'tls'],
  due: '2026-09-30',
  createdAt: '2026-09-01T09:30:00.000Z',
  updatedAt: '2026-09-12T16:30:00.000Z',
};

const ACCOUNTS: Choices = {
  data: [
    { value: ACCOUNT_ID, label: 'Root account' },
    { value: '01J8A0000000000000000000A2', label: 'Acme Ltd' },
  ],
  more: false,
};

const CALLS: Record<string, CannedAnswer> = {
  'note.get': okEnvelope(NOTE),
  'note.recompute': okEnvelope({ values: { due: DUE } }),
  'note.edit': okEnvelope({ ...NOTE, kind: 'memo', due: null }),
  'keel.account.options': okEnvelope(ACCOUNTS),
};

const UNNAMED_CONTROLS = {
  config: {
    rules: [
      { id: 'label', enabled: false },
      { id: 'label-title-only', enabled: false },
      { id: 'button-name', enabled: false },
    ],
  },
};

function control(canvas: ReturnType<typeof within>, name: string, role = 'textbox'): HTMLElement {
  return within(canvas.getByRole('group', { name })).getByRole(role);
}

const meta: Meta<FormArgs> = {
  title: 'Renderer/Form',
  decorators: [moduleMetadata({ imports: [KeelScreenComponent] })],
  parameters: { layout: 'padded', a11y: UNNAMED_CONTROLS },
  argTypes: {
    func: { control: false },
    params: { control: 'object' },
  },
  args: { func: 'note.edit', params: null },
  render: (args) => ({
    props: args,
    template: '<keel-screen [func]="func" [params]="params" />',
  }),
};

export default meta;
type Story = StoryObj<FormArgs>;

export const Create: Story = {
  decorators: [withKeel({ calls: CALLS })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Title' });
    await expect(canvas.getByRole('tab', { name: 'Note' })).toBeInTheDocument();
    await expect(canvas.getByRole('tab', { name: 'Owner' })).toBeInTheDocument();
    await expect(canvas.queryByRole('group', { name: 'Due' })).not.toBeInTheDocument();
    await userEvent.type(control(canvas, 'Title'), 'Rotate the keys');
    await userEvent.click(control(canvas, 'Kind', 'combobox'));
    const listbox = await screen.findByRole('listbox');
    await userEvent.click(within(listbox).getByRole('option', { name: 'To do' }));
    await canvas.findByRole('group', { name: 'Due' });
    await waitFor(() => expect(control(canvas, 'Due')).not.toHaveValue(''));
  },
};

export const Edit: Story = {
  args: { params: { id: NOTE_ID } },
  decorators: [withKeel({ calls: CALLS })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Title' });
    await expect(control(canvas, 'Title')).toHaveValue('Renew the certificates');
    await expect(control(canvas, 'Kind', 'combobox')).toHaveTextContent('To do');
    await expect(control(canvas, 'Due')).not.toHaveValue('');
    await expect(canvas.getByText('ops')).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Save' })).toBeInTheDocument();
  },
};

export const EditAsUser: Story = {
  name: 'Edit as a user',
  args: { params: { id: NOTE_ID } },
  decorators: [withKeel({ level: 'user', calls: CALLS })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Title' });
    await expect(control(canvas, 'Title')).toHaveValue('Renew the certificates');
    await expect(canvas.queryByRole('tab')).not.toBeInTheDocument();
    await expect(canvas.queryByRole('group', { name: 'Account' })).not.toBeInTheDocument();
  },
};

export const Refused: Story = {
  args: { params: { id: NOTE_ID } },
  decorators: [
    withKeel({
      calls: {
        ...CALLS,
        'note.edit': errorEnvelope({
          code: 'validation',
          key: 'keel.error.validation',
          fields: [
            {
              path: 'title',
              code: 'validation',
              key: 'note.error.exists',
              params: { title: 'Renew the certificates' },
            },
          ],
        }),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Title' });
    await userEvent.click(canvas.getByRole('button', { name: 'Save' }));
    await expect(
      await canvas.findByText('A note titled Renew the certificates already exists'),
    ).toBeInTheDocument();
  },
};
