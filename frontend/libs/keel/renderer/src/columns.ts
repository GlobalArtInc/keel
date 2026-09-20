import { UTC } from '@globalart/keel/client';
import {
  entityPrefix,
  lookup,
  type Choice,
  type Column,
  type ColumnType,
  type Descriptor,
  type Messages,
  type Tone,
} from '@globalart/keel/meta';
import type {
  PhxColumn,
  PhxColumnEdit,
  PhxColumnFilterOption,
  PhxColumnType,
  PhxIconValue,
  PhxStatusValue,
  PhxTableStatus,
} from '@globalart/phoenix-ui';
import type { Row } from './action/runner';
import { FALSE_KEY, TRUE_KEY } from './messages';

export const COLUMN_TYPES: Readonly<Record<ColumnType, PhxColumnType>> = {
  text: 'text',
  number: 'number',
  link: 'link',
  status: 'status',
  toggle: 'toggle',
  date: 'date',
  datetime: 'date',
  boolean: 'icon',
};

export const TONES: Readonly<Record<Tone, PhxTableStatus>> = {
  success: 'success',
  warning: 'warning',
  danger: 'danger',
  info: 'info',
  neutral: 'default',
};

export const CELL_EDIT_TYPES: readonly ColumnType[] = ['status', 'number', 'text'];

export const DAY_FORMAT: Intl.DateTimeFormatOptions = { dateStyle: 'medium' };
export const MOMENT_FORMAT: Intl.DateTimeFormatOptions = {
  dateStyle: 'medium',
  timeStyle: 'short',
};

export interface ColumnScope {
  messages: Messages;
  zone: string;
  locale: string;
  choices(func: string): readonly Choice[] | null;
}

export interface ColumnTotal {
  key: string;
  label: string;
  value: string;
}

export function columnLabel(descriptor: Descriptor, key: string, messages: Messages): string {
  const labelKey = `${entityPrefix(descriptor.name)}.column.${key}`;
  return lookup(messages, labelKey) ?? labelKey;
}

export function columnsOf(descriptor: Descriptor, scope: ColumnScope): PhxColumn<Row>[] {
  return (descriptor.ui?.columns ?? []).map((column) => toColumn(descriptor, column, scope));
}

export function toColumn(
  descriptor: Descriptor,
  column: Column,
  scope: ColumnScope,
): PhxColumn<Row> {
  const prefix = entityPrefix(descriptor.name);
  const mapped: PhxColumn<Row> = {
    key: column.key,
    label: columnLabel(descriptor, column.key, scope.messages),
    type: COLUMN_TYPES[column.type],
  };
  if (column.sortable) {
    mapped.sortable = true;
  }
  if (column.primary) {
    mapped.primary = true;
  }
  switch (column.type) {
    case 'status': {
      const map = column.map ?? {};
      const valuePrefix = `${prefix}.${column.key}`;
      mapped.value = (row) => statusOf(row[column.key], map, valuePrefix, scope.messages);
      break;
    }
    case 'boolean':
      mapped.value = (row) => flagOf(row[column.key], scope.messages);
      break;
    case 'date':
      mapped.locale = scope.locale;
      mapped.dateFormat = { ...DAY_FORMAT, timeZone: UTC };
      break;
    case 'datetime':
      mapped.locale = scope.locale;
      mapped.dateFormat = { ...MOMENT_FORMAT, timeZone: usableZone(scope.zone) };
      break;
    default:
      break;
  }
  const filter = column.filter ?? null;
  if (filter !== null) {
    mapped.filterable = true;
    mapped.filterOptions =
      'options' in filter
        ? filter.options.map((value) => staticOption(prefix, column.key, value, scope.messages))
        : (scope.choices(filter.remote) ?? []).map(remoteOption);
  }
  const edit = cellEditOf(column, prefix, scope.messages);
  if (edit !== null) {
    mapped.editable = true;
    mapped.edit = edit;
  }
  return mapped;
}

export function cellEditOf(
  column: Column,
  prefix: string,
  messages: Messages,
): PhxColumnEdit | null {
  if (!column.edit || !CELL_EDIT_TYPES.includes(column.type)) {
    return null;
  }
  if (column.type !== 'status') {
    return {};
  }
  const values = new Set<string>();
  const filter = column.filter ?? null;
  if (filter !== null && 'options' in filter) {
    for (const value of filter.options) {
      values.add(value);
    }
  }
  for (const value of Object.keys(column.map ?? {})) {
    values.add(value);
  }
  return {
    options: [...values].map((value) => staticOption(prefix, column.key, value, messages)),
  };
}

export function totalsOf(
  descriptor: Descriptor,
  totals: Record<string, unknown> | null,
  scope: Pick<ColumnScope, 'messages' | 'locale'>,
): ColumnTotal[] {
  if (totals === null) {
    return [];
  }
  const found: ColumnTotal[] = [];
  for (const column of descriptor.ui?.columns ?? []) {
    if (!column.total || !Object.hasOwn(totals, column.key)) {
      continue;
    }
    found.push({
      key: column.key,
      label: columnLabel(descriptor, column.key, scope.messages),
      value: formatTotal(totals[column.key], scope.locale),
    });
  }
  return found;
}

export function statusOf(
  value: unknown,
  map: Readonly<Record<string, Tone>>,
  valuePrefix: string,
  messages: Messages,
): PhxStatusValue {
  if (value === null || value === undefined || value === '') {
    return { status: 'default', label: '' };
  }
  const text = String(value);
  const tone = Object.hasOwn(map, text) ? map[text] : null;
  return {
    status: tone === null ? 'default' : TONES[tone],
    label: lookup(messages, `${valuePrefix}.${text}`) ?? text,
  };
}

export function flagOf(value: unknown, messages: Messages): PhxIconValue | null {
  if (typeof value !== 'boolean') {
    return null;
  }
  return value
    ? { icon: 'check', color: 'success', label: lookup(messages, TRUE_KEY) ?? TRUE_KEY }
    : { icon: 'minus', color: 'muted', label: lookup(messages, FALSE_KEY) ?? FALSE_KEY };
}

export function usableZone(zone: string): string {
  try {
    new Intl.DateTimeFormat(undefined, { timeZone: zone });
    return zone;
  } catch {
    return UTC;
  }
}

function staticOption(
  prefix: string,
  key: string,
  value: string,
  messages: Messages,
): PhxColumnFilterOption {
  return { value, label: lookup(messages, `${prefix}.${key}.${value}`) ?? value };
}

function remoteOption(choice: Choice): PhxColumnFilterOption {
  return { value: choice.value, label: choice.label };
}

function formatTotal(value: unknown, locale: string): string {
  if (typeof value === 'number') {
    try {
      return new Intl.NumberFormat(locale).format(value);
    } catch {
      return String(value);
    }
  }
  return value === null || value === undefined ? '' : String(value);
}
