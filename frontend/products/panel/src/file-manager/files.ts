import {
  KeelCallError,
  TASK_GET,
  isActive,
  isTaskRef,
  type KeelClient,
  type TaskDetail,
} from '@globalart/keel/client';
import type { CallError, Choices, Envelope, Id } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';

export const FILE_MANAGER = 'hosting.file-manager';
const FILE_LIST = 'hosting.file.list';
export const FILE_MKDIR = 'hosting.file.mkdir';
export const FILE_RENAME = 'hosting.file.rename';
export const FILE_DELETE = 'hosting.file.delete';
export const FILE_CHMOD = 'hosting.file.chmod';
const FILE_UPLOAD = 'hosting.file.upload';
export const FILE_DOWNLOAD = 'hosting.file.download';
const USER_OPTIONS = 'hosting.user.options';
const UPLOAD_FIELD = 'content';
const UPLOAD_TYPE = 'application/octet-stream';
const TASK_POLL_STEP_MS = 150;
const USER_PAGE = 100;
const MODE_PATTERN = /^0?[0-7]{3}$/;
export const KEYS = {
  title: `${FILE_MANAGER}.title`,
  user: `${FILE_MANAGER}.user`,
  pick: `${FILE_MANAGER}.pick`,
  none: `${FILE_MANAGER}.none`,
  home: `${FILE_MANAGER}.home`,
  empty: `${FILE_MANAGER}.empty`,
  truncated: `${FILE_MANAGER}.truncated`,
  columnName: `${FILE_MANAGER}.column.name`,
  columnSize: `${FILE_MANAGER}.column.size`,
  columnMode: `${FILE_MANAGER}.column.mode`,
  columnModified: `${FILE_MANAGER}.column.modified`,
  mkdir: `${FILE_MANAGER}.action.mkdir`,
  upload: `${FILE_MANAGER}.action.upload`,
  open: `${FILE_MANAGER}.action.open`,
  download: `${FILE_MANAGER}.action.download`,
  rename: `${FILE_MANAGER}.action.rename`,
  chmod: `${FILE_MANAGER}.action.chmod`,
  delete: `${FILE_MANAGER}.action.delete`,
  cancel: `${FILE_MANAGER}.action.cancel`,
  save: `${FILE_MANAGER}.action.save`,
  mkdirTitle: `${FILE_MANAGER}.mkdir.title`,
  mkdirName: `${FILE_MANAGER}.mkdir.name`,
  renameTitle: `${FILE_MANAGER}.rename.title`,
  renameName: `${FILE_MANAGER}.rename.name`,
  chmodTitle: `${FILE_MANAGER}.chmod.title`,
  chmodMode: `${FILE_MANAGER}.chmod.mode`,
  deleteTitle: `${FILE_MANAGER}.delete.title`,
  deleteText: `${FILE_MANAGER}.delete.text`,
  uploaded: `${FILE_MANAGER}.uploaded`,
  invalidName: `${FILE_MANAGER}.invalid.name`,
  invalidMode: `${FILE_MANAGER}.invalid.mode`,
} as const;

export type FileKind = 'dir' | 'file' | 'symlink' | 'other';

export interface FileEntry {
  name: string;
  kind: FileKind;
  size: number;
  mode: string;
  modified?: string | null;
}

export interface Listing {
  entries: FileEntry[];
  truncated: boolean;
}

export interface UserChoice {
  value: Id;
  label: string;
}

export interface Crumb {
  label: string;
  path: string;
}

export interface EntryQuery {
  offset: number;
  limit: number;
  search: string;
}

const KIND_ORDER: Record<FileKind, number> = { dir: 0, symlink: 1, file: 2, other: 3 };
const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
const STEP = 1024;

export function joinPath(dir: string, name: string): string {
  return dir === '' ? name : `${dir}/${name}`;
}

export function crumbsOf(path: string, home: string): Crumb[] {
  const crumbs: Crumb[] = [{ label: home, path: '' }];
  let walked = '';
  for (const part of path.split('/').filter((part) => part !== '')) {
    walked = joinPath(walked, part);
    crumbs.push({ label: part, path: walked });
  }
  return crumbs;
}

export function validName(name: string): boolean {
  return (
    name !== '' && name !== '.' && name !== '..' && !name.includes('/') && !name.includes('\0')
  );
}

export function validMode(mode: string): boolean {
  return MODE_PATTERN.test(mode);
}

export function sizeOf(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= STEP && unit < UNITS.length - 1) {
    value /= STEP;
    unit += 1;
  }
  return unit === 0 ? `${bytes} ${UNITS[0]}` : `${value.toFixed(1)} ${UNITS[unit]}`;
}

export function sorted(entries: readonly FileEntry[]): FileEntry[] {
  return [...entries].sort(
    (a, b) => KIND_ORDER[a.kind] - KIND_ORDER[b.kind] || a.name.localeCompare(b.name),
  );
}

export function pageOf(
  entries: readonly FileEntry[],
  query: EntryQuery,
): { data: FileEntry[]; total: number } {
  const needle = query.search.trim().toLowerCase();
  const matching = sorted(entries).filter(
    (entry) => needle === '' || entry.name.toLowerCase().includes(needle),
  );
  return { data: matching.slice(query.offset, query.offset + query.limit), total: matching.length };
}

function settled<T>(envelope: Envelope<T>): T {
  if (!envelope.ok) {
    throw new KeelCallError(envelope.error);
  }
  return envelope.data;
}

function failureOf(detail: TaskDetail): CallError {
  return (
    detail.error ?? {
      code: 'internal',
      key: 'keel.error.internal',
    }
  );
}

export class FileApi {
  constructor(
    private readonly client: KeelClient,
    private readonly step = TASK_POLL_STEP_MS,
  ) {}

  async users(): Promise<UserChoice[]> {
    const choices = settled(
      await firstValueFrom(this.client.call<Choices>(USER_OPTIONS, { limit: USER_PAGE })),
    );
    return choices.data.map((choice) => ({ value: choice.value, label: choice.label }));
  }

  async list(user: Id, path: string): Promise<Listing> {
    return settled(await firstValueFrom(this.client.call<Listing>(FILE_LIST, { user, path })));
  }

  async change(func: string, params: Record<string, unknown>): Promise<void> {
    await this.follow(await firstValueFrom(this.client.call(func, params)));
  }

  async upload(user: Id, path: string, file: File): Promise<void> {
    const content = new File([file], file.name, { type: UPLOAD_TYPE });
    await this.follow(
      await firstValueFrom(
        this.client.upload(FILE_UPLOAD, { user, path }, { [UPLOAD_FIELD]: [content] }),
      ),
    );
  }

  private async follow(envelope: Envelope): Promise<void> {
    const data = settled(envelope);
    if (!isTaskRef(data)) {
      return;
    }
    for (;;) {
      const detail = settled(
        await firstValueFrom(this.client.call<TaskDetail>(TASK_GET, { id: data.task.id })),
      );
      if (!isActive(detail.state)) {
        if (detail.state !== 'done') {
          throw new KeelCallError(failureOf(detail));
        }
        return;
      }
      await new Promise((done) => setTimeout(done, this.step));
    }
  }
}
