export type Level = 'guest' | 'service' | 'user' | 'reseller' | 'admin' | 'owner';
export type Id = string;
export type AccountState = 'active' | 'suspended' | 'deleting';
export type ActionRole =
  | 'new'
  | 'edit'
  | 'delete'
  | 'group'
  | 'button'
  | 'child'
  | 'external'
  | 'export'
  | 'upload'
  | 'download'
  | 'refresh';
export type Confirm = boolean | ConfirmDetails;
export type ConfirmInput = 'count';
export type Expr =
  | {
      eq: [string, unknown];
    }
  | {
      ne: [string, unknown];
    }
  | {
      gt: [string, unknown];
    }
  | {
      lt: [string, unknown];
    }
  | {
      in: [string, unknown[]];
    }
  | {
      matches: [string, string];
    }
  | {
      truthy: string;
    }
  | {
      empty: string;
    }
  | {
      and: Expr[];
    }
  | {
      or: Expr[];
    }
  | {
      not: Expr;
    };
export type ExportFormat = 'csv' | 'json';
export type ActorVia = 'su' | 'socket' | 'peer';
export type Kind =
  | 'list'
  | 'get'
  | 'form'
  | 'action'
  | 'wizard'
  | 'report'
  | 'options'
  | 'stream'
  | 'download'
  | 'upload';
export type ErrorCode =
  | 'validation'
  | 'forbidden'
  | 'forbidden_field'
  | 'unauthenticated'
  | 'not_found'
  | 'conflict'
  | 'idempotency_mismatch'
  | 'limit_exceeded'
  | 'license'
  | 'rate_limited'
  | 'unavailable'
  | 'peer'
  | 'internal';
export type Check = 'domain' | 'ip' | 'cidr' | 'email' | 'int' | 'name' | 'port' | 'path';
export type ColumnType =
  'text' | 'number' | 'link' | 'status' | 'toggle' | 'date' | 'datetime' | 'boolean';
export type Total = 'sum' | 'avg' | 'min' | 'max' | 'count';
export type Tone = 'success' | 'warning' | 'danger' | 'info' | 'neutral';
export type ColumnFilter =
  | {
      options: string[];
    }
  | {
      remote: string;
    };
export type Control =
  | 'input'
  | 'password'
  | 'select'
  | 'toggle'
  | 'switcher'
  | 'slider'
  | 'date'
  | 'time'
  | 'file'
  | 'list'
  | 'code'
  | 'tags';
export type Mode = 'read' | 'write';
export type ScreenResult = 'redirect' | 'task' | 'download';
export type Envelope =
  | {
      ok: true;
      data: unknown;
      meta: Meta;
    }
  | {
      ok: false;
      error: CallError;
    };
export type EventActorKind = 'principal' | 'peer' | 'system';
export type FilterOp = 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte';
export type FilterValue = string | string[];
export type SortDir = 'asc' | 'desc';
export type Scope =
  | ('full' | 'read_only')
  | {
      functions: string[];
    };
export type Via =
  | ('http' | 'socket' | 'internal' | 'su')
  | {
      plugin: string;
    }
  | {
      peer: Id;
    };
export type TaskState = 'queued' | 'running' | 'done' | 'failed' | 'cancelled' | 'interrupted';
export interface Access {
  level: Level;
  permission?: string | null;
  feature?: string | null;
}
export interface Account {
  id: Id;
  kind: string;
  parentId?: Id | null;
  path: string;
  state: AccountState;
  externalId?: string | null;
  origin?: string | null;
}
export interface Action {
  id: string;
  role: ActionRole;
  screen?: string | null;
  function?: string | null;
  icon?: string | null;
  default?: boolean;
  params?: Record<string, unknown>;
  confirm?: Confirm | null;
  hidden_if?: Expr | null;
  formats?: ExportFormat[];
}
export interface ConfirmDetails {
  destructive?: boolean;
  items?: string | null;
  input?: ConfirmInput | null;
}
export interface Badge {
  problem: string;
}
export interface Boot {
  keel: Platform;
  products: BootProduct[];
  brand: BootBrand;
  principal?: BootPrincipal | null;
  features: string[];
  license?: unknown;
  menu: BootMenuGroup[];
  dashboard: unknown[];
  views: string[];
  meta: string;
  problems?: unknown;
}
export interface Platform {
  version: string;
  api: number;
}
export interface BootProduct {
  id: string;
  name: string;
  edition?: string | null;
  brand: BootBrand;
}
export interface BootBrand {
  name: string;
  logo?: string | null;
  hue: number;
  chroma: number;
}
export interface BootPrincipal {
  id: Id;
  login: string;
  level: Level;
  account: Id;
  actor?: MetaActor | null;
  lang: string;
  tz?: string | null;
  totp: boolean;
}
export interface MetaActor {
  id: Id;
  login: string;
  level: Level;
  via: ActorVia;
}
export interface BootMenuGroup {
  id: string;
  product?: string | null;
  label: string;
  icon: string;
  items: BootMenuItem[];
}
export interface BootMenuItem {
  path: string;
  label: string;
  func: string;
  kind?: Kind | null;
  icon?: string | null;
  badge?: Badge | null;
}
export interface CallError {
  code: ErrorCode;
  key: string;
  params?: Record<string, unknown>;
  requestId?: string | null;
  fields?: FieldError[];
}
export interface FieldError {
  path: string;
  code?: string | null;
  key?: string | null;
  params?: Record<string, unknown>;
}
export interface Choice {
  value: string;
  label: string;
  hint?: string | null;
  icon?: string | null;
  group?: string | null;
}
export interface Choices {
  data: Choice[];
  more: boolean;
}
export interface Column {
  key: string;
  type: ColumnType;
  to?: Link | null;
  sortable?: boolean;
  primary?: boolean;
  total?: Total | null;
  map?: {
    [k: string]: Tone;
  } | null;
  filter?: ColumnFilter | null;
  edit?: InlineEdit | null;
  access?: MemberAccess | null;
}
export interface Link {
  screen: string;
  params?: Record<string, unknown>;
}
export interface InlineEdit {
  function: string;
}
export interface MemberAccess {
  level?: Level | null;
  permission?: string | null;
}
export interface Context {
  fields?: Record<string, unknown>;
  mode?: string | null;
  principal?: ContextPrincipal | null;
  screen?: ContextScreen | null;
}
export interface ContextPrincipal {
  level: Level;
}
export interface ContextScreen {
  params?: Record<string, unknown>;
}
export interface Descriptor {
  keel: number;
  name: string;
  module: string;
  kind: Kind;
  mode: Mode;
  access: Access;
  key?: string | null;
  params: Record<string, unknown> | boolean;
  result: Record<string, unknown> | boolean;
  ui?: ScreenUi | null;
}
export interface ScreenUi {
  title: string;
  search?: string[];
  columns?: Column[];
  actions?: Action[];
  row_actions?: string[];
  empty?: Empty | null;
  invalidate_on?: string[];
  read?: string | null;
  pages?: ScreenPage[];
  fields?: {
    [k: string]: FieldUi;
  };
  multi?: string[];
  result?: ScreenResult | null;
}
export interface Empty {
  title: string;
  action?: string | null;
}
export interface ScreenPage {
  id: string;
  fields?: string[];
  show_if?: Expr | null;
  validate?: string | null;
}
export interface FieldUi {
  show_if?: Expr | null;
  required_if?: Expr | null;
  readonly_if?: Expr | null;
  recompute?: string | null;
}
export interface Meta {
  requestId: string;
  principal?: MetaPrincipal | null;
}
export interface MetaPrincipal {
  id: Id;
  login: string;
  level: Level;
  account: Id;
  actor?: MetaActor | null;
}
export interface Event {
  id: Id;
  seq: number;
  type: string;
  at: string;
  instanceId: Id;
  module: string;
  actor?: EventActor | null;
  subject?: EventSubject | null;
  data?: unknown;
}
export interface EventActor {
  kind: EventActorKind;
  id: Id;
}
export interface EventSubject {
  kind: string;
  id: Id;
  accountId?: Id | null;
}
export interface Filter {
  key: string;
  op?: FilterOp;
  value: FilterValue;
}
export interface KeelAttrs {
  level?: Level | null;
  secret?: boolean;
  once?: boolean;
  check?: Check | null;
  options?: string | null;
}
export interface ListQuery {
  page?: number;
  pageSize?: number;
  search?: string;
  sort?: Sort | null;
  filters?: Filter[];
  scope?: Record<string, unknown>;
}
export interface Sort {
  key: string;
  dir: SortDir;
}
export interface OptionsQuery {
  search?: string;
  scope?: Record<string, unknown>;
  limit?: number;
  cursor?: string | null;
}
export interface Page {
  data: unknown[];
  total: number;
  totals?: Record<string, unknown> | null;
}
export interface Principal {
  id: Id;
  login: string;
  level: Level;
  accountId: Id;
  role: string;
  actor?: Principal | null;
  scope: Scope;
  via: Via;
  externalId?: string | null;
  origin?: string | null;
  lang: string;
}
export interface TaskProgress {
  id: Id;
  function: string;
  state: TaskState;
  done: number;
  total?: number | null;
  message?: string | null;
}
export interface UiAttrs {
  control?: Control | null;
  placeholder?: string | null;
  help?: string | null;
  order?: number | null;
  generate?: boolean;
}
