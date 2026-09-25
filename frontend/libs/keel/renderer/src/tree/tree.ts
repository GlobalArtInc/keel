import { isObject } from '@globalart/keel/forms';
import type { Descriptor, ListQuery } from '@globalart/keel/meta';
import type { PhxTreeNode } from '@globalart/phoenix-ui';
import { keyOf, type Row } from '../action/runner';
import { scopeOf } from '../list/scope';

const LEVEL_SIZE = 200;

export function parentOf(descriptor: Descriptor): string | null {
  const ui = descriptor.ui;
  return ui?.layout === 'tree' ? (ui.parent ?? null) : null;
}

function labelColumnOf(descriptor: Descriptor): string {
  const columns = descriptor.ui?.columns ?? [];
  return (
    (columns.find((column) => column.primary === true) ?? columns[0])?.key ?? keyOf(descriptor)
  );
}

export function levelQuery(
  parent: string,
  under: string | null,
  params: Record<string, unknown>,
): ListQuery {
  return { page: 1, pageSize: LEVEL_SIZE, scope: { ...scopeOf(params), [parent]: under } };
}

function text(value: unknown, fallback: string): string {
  if (typeof value === 'string' && value !== '') {
    return value;
  }
  return typeof value === 'number' || typeof value === 'boolean' ? String(value) : fallback;
}

export function nodesOf(descriptor: Descriptor, rows: readonly Row[]): PhxTreeNode[] {
  const key = keyOf(descriptor);
  const label = labelColumnOf(descriptor);
  return rows.map((row) => {
    const id = String(row[key]);
    return { id, label: text(row[label], id), hasChildren: true, data: row };
  });
}

export function rowOf(node: PhxTreeNode): Row {
  return isObject(node.data) ? node.data : {};
}
