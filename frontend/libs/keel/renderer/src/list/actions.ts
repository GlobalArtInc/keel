import { holds, isIconName } from '@globalart/keel/forms';
import type {
  Action,
  ActionRole,
  Context,
  ContextPrincipal,
  Descriptor,
} from '@globalart/keel/meta';
import type { PhxButtonAppearance, PhxIconName } from '@globalart/phoenix-ui';
import { ROW_PLACEHOLDER, keyOf, resolveTokens, type Row } from '../action/runner';

export const BULK_ROLES: readonly ActionRole[] = ['delete', 'group'];
export const SELECTION_ROLES: readonly ActionRole[] = [
  'edit',
  'delete',
  'group',
  'button',
  'child',
  'download',
];
export const EXPORT_FUNCTION = 'keel.export';

const ROLE_ICONS: Readonly<Partial<Record<ActionRole, PhxIconName>>> = {
  new: 'plus',
  edit: 'pencil',
  delete: 'trash-2',
  refresh: 'refresh-cw',
  export: 'download',
  upload: 'upload',
  download: 'download',
  external: 'external-link',
};

export interface RowScope {
  principal: ContextPrincipal | null;
  params: Record<string, unknown>;
}

export function actionsOf(descriptor: Descriptor): readonly Action[] {
  return descriptor.ui?.actions ?? [];
}

export function actionOf(descriptor: Descriptor, id: string): Action | null {
  return actionsOf(descriptor).find((action) => action.id === id) ?? null;
}

export function isBulk(action: Action): boolean {
  return BULK_ROLES.includes(action.role);
}

export function rowActionsOf(descriptor: Descriptor): Action[] {
  const found: Action[] = [];
  for (const id of descriptor.ui?.row_actions ?? []) {
    const action = actionOf(descriptor, id);
    if (action !== null) {
      found.push(action);
    }
  }
  return found;
}

export function bulkActionsOf(descriptor: Descriptor): Action[] {
  return actionsOf(descriptor).filter(isBulk);
}

export function selectionActionsOf(descriptor: Descriptor, multiEdit: Action | null): Action[] {
  const found = new Set<Action>(
    rowActionsOf(descriptor).filter(
      (action) => SELECTION_ROLES.includes(action.role) || Boolean(action.confirm),
    ),
  );
  for (const action of bulkActionsOf(descriptor)) {
    found.add(action);
  }
  if (multiEdit !== null) {
    found.add(multiEdit);
  }
  return [...found];
}

export function isSelectionBulk(action: Action, multiEdit: Action | null): boolean {
  return isBulk(action) || action === multiEdit;
}

export function toolbarActionsOf(descriptor: Descriptor): Action[] {
  const onRows = new Set(descriptor.ui?.row_actions ?? []);
  return actionsOf(descriptor).filter((action) => !onRows.has(action.id) && !isBulk(action));
}

export function defaultActionOf(descriptor: Descriptor): Action | null {
  return actionsOf(descriptor).find((action) => action.default === true) ?? null;
}

export function emptyActionOf(descriptor: Descriptor): Action | null {
  const id = descriptor.ui?.empty?.action ?? null;
  return id === null ? null : actionOf(descriptor, id);
}

export function rowContext(row: Row, scope: RowScope): Context {
  return { fields: row, principal: scope.principal, screen: { params: scope.params } };
}

export function isHidden(action: Action, row: Row, scope: RowScope): boolean {
  return holds(action.hidden_if, rowContext(row, scope), false);
}

export function screenParamsOf(
  descriptor: Descriptor,
  action: Action,
  screen: Record<string, unknown>,
  row?: Row,
): Record<string, unknown> {
  const params =
    action.params ?? (row === undefined ? {} : { [keyOf(descriptor)]: ROW_PLACEHOLDER });
  return resolveTokens(params, { row, key: keyOf(descriptor), screen });
}

export function iconOf(action: Action): PhxIconName | null {
  return isIconName(action.icon) ? action.icon : (ROLE_ICONS[action.role] ?? null);
}

export function appearanceOf(action: Action): PhxButtonAppearance {
  switch (action.role) {
    case 'new':
      return 'primary';
    case 'delete':
      return 'danger';
    default:
      return 'default';
  }
}
