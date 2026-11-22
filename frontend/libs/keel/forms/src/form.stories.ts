import type { Choices } from '@globalart/keel/meta';
import {
  ACCOUNT_ID,
  KEEL_BOOT,
  descriptorOf,
  keelDump,
  okEnvelope,
  withKeel,
  type CannedAnswer,
} from '@globalart/keel/testing';
import { moduleMetadata, type Meta, type StoryObj } from '@storybook/angular';
import { expect, waitFor } from 'storybook/test';
import { KeelFormComponent } from './form.component';
import { buildForm } from './model';

const ACCOUNTS: Choices = {
  data: [
    { value: ACCOUNT_ID, label: 'Root account' },
    { value: '01J8A0000000000000000000A2', label: 'Acme Ltd' },
  ],
  more: false,
};

const CALLS: Record<string, CannedAnswer> = {
  'keel.account.options': okEnvelope(ACCOUNTS),
};

const ROW_GAP = '28px';
const MIN_ROW_GAP = 20;
const MIN_TAB_OFFSET = 16;

function tabPage(root: HTMLElement): HTMLElement {
  const page = root.querySelector<HTMLElement>('phx-tab:not([hidden]) keel-page');
  if (page === null) {
    throw new Error('no visible tab page');
  }
  return page;
}

function rowsOf(page: HTMLElement): HTMLElement[] {
  return [...page.querySelectorAll<HTMLElement>(':scope > keel-field')];
}

function tabStrip(root: HTMLElement): HTMLElement {
  const strip = root.querySelector<HTMLElement>('phx-tab-group [role="tablist"]');
  if (strip === null) {
    throw new Error('no tab strip');
  }
  return strip;
}

const meta: Meta = {
  title: 'Forms/Form',
  decorators: [moduleMetadata({ imports: [KeelFormComponent] })],
  parameters: { layout: 'padded' },
  render: () => ({
    props: {
      form: buildForm(descriptorOf(keelDump('admin'), 'note.edit'), { values: {} }),
    },
    template: '<div style="width: 1200px"><keel-form [form]="form" /></div>',
  }),
};

export default meta;
type Story = StoryObj;

export const TabsLayout: Story = {
  decorators: [withKeel({ boot: KEEL_BOOT.one, calls: CALLS })],
  play: async ({ canvasElement }) => {
    await waitFor(() => expect(tabPage(canvasElement)).toBeTruthy());
    const page = tabPage(canvasElement);
    const style = getComputedStyle(page);
    await expect(style.display).toBe('flex');
    await expect(style.rowGap).toBe(ROW_GAP);

    const [first, second] = rowsOf(page);
    if (first === undefined || second === undefined) {
      throw new Error('the first page has fewer than two rows');
    }
    const gap = second.getBoundingClientRect().top - first.getBoundingClientRect().bottom;
    await expect(gap).toBeGreaterThanOrEqual(MIN_ROW_GAP);

    const offset =
      first.getBoundingClientRect().top - tabStrip(canvasElement).getBoundingClientRect().bottom;
    await expect(offset).toBeGreaterThanOrEqual(MIN_TAB_OFFSET);
  },
};
