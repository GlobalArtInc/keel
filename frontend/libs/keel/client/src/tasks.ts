import type {
  CallError,
  DownloadRef,
  Id,
  ListQuery,
  TaskRef,
  TaskState,
} from '@globalart/keel/meta';

export const TASK_GET = 'task.get';
export const TASK_LIST = 'task.list';
export const TASK_CANCEL = 'task.cancel';
export const TASK_PAGE_SIZE = 50;
export const ACTIVE_STATES: readonly TaskState[] = ['queued', 'running'];

export interface TaskRow {
  id: Id;
  function: string;
  class: string;
  state: TaskState;
  progress: string;
  done: number;
  total?: number | null;
  message?: string | null;
  resumable: boolean;
  requestId?: string | null;
  createdAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  accountId: Id;
  principalId: Id;
}

export interface TaskDetail extends TaskRow {
  params: unknown;
  checkpoint?: unknown;
  result?: unknown;
  error?: CallError | null;
  logTail: string[];
}

function isRecord(data: unknown): data is Record<string, unknown> {
  return typeof data === 'object' && data !== null && !Array.isArray(data);
}

export function isTaskRef(data: unknown): data is TaskRef {
  return isRecord(data) && isRecord(data['task']) && typeof data['task']['id'] === 'string';
}

export function isDownloadRef(data: unknown): data is DownloadRef {
  return (
    isRecord(data) &&
    isRecord(data['download']) &&
    typeof data['download']['function'] === 'string' &&
    isRecord(data['download']['params'])
  );
}

export function isActive(state: TaskState): boolean {
  return ACTIVE_STATES.includes(state);
}

export function recentTasks(pageSize = TASK_PAGE_SIZE): ListQuery {
  return { page: 1, pageSize, sort: { key: 'createdAt', dir: 'desc' } };
}
