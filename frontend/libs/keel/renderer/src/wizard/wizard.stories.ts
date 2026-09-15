import type { Choices } from '@globalart/keel/meta';
import {
  ACCOUNT_ID,
  errorEnvelope,
  okEnvelope,
  withKeel,
  type CannedAnswer,
} from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, userEvent, within } from 'storybook/test';
import { KeelScreenComponent } from '../screen';

interface WizardArgs {
  func: string;
  params: Record<string, unknown> | null;
}

const LINES = 'Renew the certificates\nRelease notes 0.1';

const ACCOUNTS: Choices = {
  data: [
    { value: ACCOUNT_ID, label: 'Root account' },
    { value: '01J8A0000000000000000000A2', label: 'Acme Ltd' },
  ],
  more: false,
};

const CALLS: Record<string, CannedAnswer> = {
  'note.import.check': okEnvelope({ lines: 2 }),
  'note.import': okEnvelope({
    imported: 2,
    ids: ['01J8A0000000000000000000N4', '01J8A0000000000000000000N5'],
  }),
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

const meta: Meta<WizardArgs> = {
  title: 'Renderer/Wizard',
  decorators: [moduleMetadata({ imports: [KeelScreenComponent] })],
  parameters: { layout: 'padded', a11y: UNNAMED_CONTROLS },
  argTypes: {
    func: { control: false },
    params: { control: 'object' },
  },
  args: { func: 'note.import', params: null },
  render: (args) => ({
    props: args,
    template: '<keel-screen [func]="func" [params]="params" />',
  }),
};

export default meta;
type Story = StoryObj<WizardArgs>;

export const Import: Story = {
  decorators: [withKeel({ calls: CALLS })],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Lines' });
    await expect(canvas.getByRole('heading', { name: 'Import notes' })).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Back' })).toBeDisabled();
    await userEvent.type(control(canvas, 'Lines'), LINES);
    await userEvent.click(canvas.getByRole('button', { name: 'Next' }));
    await expect(await canvas.findByRole('group', { name: 'Kind' })).toBeInTheDocument();
    await expect(control(canvas, 'Kind', 'combobox')).toHaveTextContent('Memo');
    await expect(canvas.getByRole('button', { name: 'Finish' })).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Back' })).toBeEnabled();
  },
};

export const RefusedLine: Story = {
  name: 'Refused line',
  decorators: [
    withKeel({
      calls: {
        ...CALLS,
        'note.import.check': errorEnvelope({
          code: 'validation',
          key: 'keel.error.validation',
          fields: [
            {
              path: 'text',
              code: 'validation',
              key: 'note.error.line_too_long',
              params: { line: 2, max: 200 },
            },
          ],
        }),
      },
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Lines' });
    await userEvent.type(control(canvas, 'Lines'), LINES);
    await userEvent.click(canvas.getByRole('button', { name: 'Next' }));
    await expect(
      await canvas.findByText('Line 2 is longer than 200 characters'),
    ).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Next' })).toBeInTheDocument();
  },
};
