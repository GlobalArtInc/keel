import { chipsOf, type ChipColumn } from './list/chips';

const COLUMNS: ChipColumn[] = [
  {
    key: 'status',
    label: 'Status',
    filterOptions: [
      { value: 'active', label: 'Active' },
      { value: 'suspended', label: 'Suspended' },
    ],
  },
  { key: 'account', label: 'Account', filterOptions: [{ value: 'acme', label: 'ACME Ltd' }] },
];

const OPTIONS = { separator: ', ' };

describe('the filter chips', () => {
  it('a static option shows its label under the column label', () => {
    expect(chipsOf(COLUMNS, [{ key: 'status', value: 'active' }], OPTIONS)).toEqual([
      { key: 'status', label: 'Status', value: 'Active' },
    ]);
  });

  it('a value that no longer is an option shows as it is', () => {
    expect(chipsOf(COLUMNS, [{ key: 'status', value: 'archived' }], OPTIONS)).toEqual([
      { key: 'status', label: 'Status', value: 'archived' },
    ]);
  });

  it('several values join with the separator and a filter of an unknown column keeps its key', () => {
    expect(
      chipsOf(
        COLUMNS,
        [
          { key: 'account', value: ['acme', 'globe'] },
          { key: 'owner', value: 'ops' },
        ],
        OPTIONS,
      ),
    ).toEqual([
      { key: 'account', label: 'Account', value: 'ACME Ltd, globe' },
      { key: 'owner', label: 'owner', value: 'ops' },
    ]);
  });
});
