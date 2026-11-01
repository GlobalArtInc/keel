import type { PhxColumn, PhxColumnFilter, PhxFilterChip } from '@globalart/phoenix-ui';

export type ChipColumn = Pick<PhxColumn, 'key' | 'label' | 'filterOptions'>;

export interface ChipOptions {
  separator: string;
}

export function chipsOf(
  columns: readonly ChipColumn[],
  filters: readonly PhxColumnFilter[],
  options: ChipOptions,
): PhxFilterChip[] {
  return filters.map((filter) => {
    const column = columns.find((candidate) => candidate.key === filter.key);
    const values = Array.isArray(filter.value) ? filter.value : [filter.value];
    const names = values.map(
      (value) => column?.filterOptions?.find((option) => option.value === value)?.label ?? value,
    );
    return {
      key: filter.key,
      label: column?.label ?? filter.key,
      value: names.join(options.separator),
    };
  });
}
