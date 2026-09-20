import { ACCOUNT_ID, okEnvelope, pageEnvelope, withKeel } from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, userEvent, waitFor, within } from 'storybook/test';
import { KeelScreenComponent } from '../screen';

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

function id(suffix: string): string {
  return `01J8A${'0'.repeat(21 - suffix.length)}${suffix}`;
}

const NOTES: Note[] = [
  {
    id: id('N1'),
    accountId: ACCOUNT_ID,
    title: 'Renew the certificates',
    body: '',
    kind: 'memo',
    tags: ['ops'],
    due: null,
    createdAt: '2026-09-14T09:00:00Z',
    updatedAt: '2026-09-14T09:00:00Z',
  },
  {
    id: id('N2'),
    accountId: ACCOUNT_ID,
    title: 'Release notes 0.1',
    body: '',
    kind: 'memo',
    tags: ['docs'],
    due: null,
    createdAt: '2026-09-14T10:00:00Z',
    updatedAt: '2026-09-14T10:00:00Z',
  },
];

const meta: Meta = {
  title: 'Renderer/MultiForm',
  decorators: [
    moduleMetadata({ imports: [KeelScreenComponent] }),
    withKeel({
      calls: {
        'note.list': pageEnvelope(NOTES),
        'note.edit': okEnvelope({ updated: 2 }),
        'keel.account.options': okEnvelope({ data: [], more: false }),
      },
    }),
  ],
  parameters: { layout: 'padded' },
  render: () => ({ template: '<keel-screen func="note.list" />' }),
};

export default meta;
type Story = StoryObj;

export const GroupEdit: Story = {
  name: 'Two notes edited as a group',
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Renew the certificates');
    const boxes = canvas.getAllByRole('checkbox', { name: /Select row/ });
    await userEvent.click(boxes[0]);
    await userEvent.click(boxes[1]);
    await userEvent.click(await canvas.findByRole('button', { name: /^Edit$/ }));

    const body = within(document.body);
    const modal = await body.findByRole('dialog', { name: 'Edit 2 rows' });
    const rows = within(modal);
    await expect(rows.getByRole('switch', { name: 'Change: Kind' })).toBeInTheDocument();
    await expect(rows.getByRole('switch', { name: 'Change: Due' })).toBeInTheDocument();
    await expect(rows.getByRole('switch', { name: 'Change: Tags' })).toBeInTheDocument();
    const save = rows.getByRole('button', { name: 'Save' });
    await expect(save).toBeDisabled();

    const control = modal.querySelector<HTMLElement>('[data-multi="kind"] phx-toggle .control');
    if (control === null) {
      throw new Error('no toggle control for kind');
    }
    await userEvent.click(control);
    await expect(rows.getByRole('switch', { name: 'Change: Kind' })).toBeChecked();
    await userEvent.click(rows.getByRole('combobox'));
    await userEvent.click(await body.findByRole('option', { name: 'To do' }));
    await expect(save).toBeEnabled();
    await userEvent.click(save);
    await waitFor(() => expect(body.queryByRole('dialog', { name: 'Edit 2 rows' })).toBeNull());
    await expect(await body.findByText('2 rows updated')).toBeInTheDocument();
  },
};
