import { ChangeDetectionStrategy, Component, computed, inject, signal } from '@angular/core';
import { KeelCallError, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { CallError, Id } from '@globalart/keel/meta';
import {
  KeelDownloads,
  RETRY_KEY,
  describeError,
  injectKeelViewContext,
} from '@globalart/keel/renderer';
import {
  PhxBreadcrumbsComponent,
  PhxButtonComponent,
  PhxConfirmService,
  PhxDataTableComponent,
  PhxEmptyComponent,
  PhxErrorComponent,
  PhxIconComponent,
  PhxLoaderComponent,
  PhxModalService,
  PhxPageHeaderComponent,
  PhxPanelComponent,
  PhxSelectComponent,
  PhxTableCellDirective,
  PhxToastService,
  type PhxColumn,
  type PhxCrumb,
  type PhxCrumbSelect,
  type PhxDataTableFetch,
  type PhxRowAction,
  type PhxRowActionEvent,
  type PhxRowEvent,
  type PhxSelectOption,
} from '@globalart/phoenix-ui';
import { Subject, firstValueFrom, of } from 'rxjs';
import {
  FILE_CHMOD,
  FILE_DELETE,
  FILE_DOWNLOAD,
  FILE_MKDIR,
  FILE_RENAME,
  FileApi,
  KEYS,
  crumbsOf,
  joinPath,
  pageOf,
  sizeOf,
  validMode,
  validName,
  type FileEntry,
  type UserChoice,
} from './files';
import { FilePromptComponent, type PromptData } from './prompt.component';

export type FilesState =
  | { phase: 'loading' }
  | { phase: 'failed'; error: CallError }
  | { phase: 'loaded'; entries: FileEntry[]; truncated: boolean };

export type UsersState =
  | { phase: 'loading' }
  | { phase: 'failed'; error: CallError }
  | { phase: 'loaded'; users: UserChoice[] };

const PROMPT_WIDTH = 400;
const LIST_LIMIT = 5000;

function callErrorOf(error: unknown): CallError {
  return error instanceof KeelCallError ? error.error : unreachable(error);
}

function paramOf(params: Record<string, unknown>, name: string): string | null {
  const value = params[name];
  return typeof value === 'string' && value !== '' ? value : null;
}

@Component({
  selector: 'panel-file-manager',
  imports: [
    PhxBreadcrumbsComponent,
    PhxButtonComponent,
    PhxDataTableComponent,
    PhxEmptyComponent,
    PhxErrorComponent,
    PhxIconComponent,
    PhxLoaderComponent,
    PhxPageHeaderComponent,
    PhxPanelComponent,
    PhxSelectComponent,
    PhxTableCellDirective,
  ],
  template: `
    <phx-page-header [title]="t(keys.title)" />
    <phx-panel>
      @switch (users().phase) {
        @case ('loading') {
          <div class="loading"><phx-loader [size]="24" /></div>
        }
        @case ('failed') {
          <phx-error [text]="usersMessage()" [retryLabel]="t(retry)" (retry)="loadUsers()" />
        }
        @case ('loaded') {
          @if (choices().length === 0) {
            <phx-empty icon="users" [title]="t(keys.none)" [panel]="false" />
          } @else {
            <div class="bar">
              <phx-select
                class="user"
                [label]="t(keys.user)"
                [options]="choices()"
                [value]="user()"
                (change)="pick($event)"
              />
              @if (user() !== null) {
                <div class="actions">
                  <button
                    phxButton
                    type="button"
                    icon="folder-plus"
                    class="mkdir"
                    (click)="mkdir()"
                  >
                    {{ t(keys.mkdir) }}
                  </button>
                  <button
                    phxButton="primary"
                    type="button"
                    icon="upload"
                    class="upload"
                    [loading]="busy()"
                    (click)="picker.click()"
                  >
                    {{ t(keys.upload) }}
                  </button>
                  <input
                    #picker
                    type="file"
                    multiple
                    hidden
                    data-picker="upload"
                    (change)="upload($event)"
                  />
                </div>
              }
            </div>
            @if (user() === null) {
              <phx-empty icon="folder" [title]="t(keys.pick)" [panel]="false" />
            } @else {
              <phx-breadcrumbs
                class="crumbs"
                [crumbs]="crumbs()"
                [label]="t(keys.title)"
                (navigate)="go($event)"
              />
              @switch (files().phase) {
                @case ('loading') {
                  <div class="loading"><phx-loader [size]="24" /></div>
                }
                @case ('failed') {
                  <phx-error [text]="filesMessage()" [retryLabel]="t(retry)" (retry)="load()" />
                }
                @case ('loaded') {
                  @if (truncated()) {
                    <p class="truncated">{{ truncatedText() }}</p>
                  }
                  <phx-data-table
                    [columns]="columns()"
                    [fetch]="fetch"
                    rowKey="name"
                    [label]="t(keys.title)"
                    [rowActions]="rowActions()"
                    [reload]="reload"
                    [reorderable]="false"
                    [emptyTitle]="t(keys.empty)"
                    emptyIcon="folder"
                    clickable
                    (rowClick)="open($event)"
                    (rowAction)="act($event)"
                  >
                    <ng-template phxTableCell="name" let-row="row">
                      @let item = entryOf(row);
                      <span class="name" [attr.data-entry]="item.name" [attr.data-kind]="item.kind">
                        <phx-icon [name]="item.kind === 'dir' ? 'folder' : 'file'" [size]="16" />
                        {{ item.name }}
                      </span>
                    </ng-template>
                  </phx-data-table>
                }
              }
            }
          }
        }
      }
    </phx-panel>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 16px;
      min-height: 0;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0;
    }
    .bar {
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
      align-items: flex-end;
      justify-content: space-between;
      margin-bottom: 12px;
    }
    .user {
      min-width: 240px;
    }
    .actions {
      display: flex;
      gap: 8px;
    }
    .crumbs {
      display: block;
      margin-bottom: 8px;
    }
    .name {
      display: inline-flex;
      gap: 8px;
      align-items: center;
    }
    .truncated {
      margin: 0 0 8px;
      color: var(--phx-text-muted);
    }
  `,
  host: {
    'data-view': 'hosting.file-manager',
    '[attr.data-phase]': 'files().phase',
    '[attr.data-path]': 'path()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class FileManagerComponent {
  readonly context = injectKeelViewContext();
  readonly keys = KEYS;
  readonly retry = RETRY_KEY;

  private readonly i18n = inject(KeelI18n);
  private readonly modals = inject(PhxModalService);
  private readonly confirms = inject(PhxConfirmService);
  private readonly toasts = inject(PhxToastService);
  private readonly downloads = inject(KeelDownloads);
  private readonly api = new FileApi(this.context.client);
  private readonly heldUsers = signal<UsersState>({ phase: 'loading' });
  private readonly heldFiles = signal<FilesState>({ phase: 'loading' });
  private readonly heldUser = signal<Id | null>(paramOf(this.context.params, 'user'));
  private readonly heldPath = signal(paramOf(this.context.params, 'path') ?? '');
  private readonly working = signal(false);
  private sequence = 0;

  readonly reload = new Subject<void>();
  readonly users = this.heldUsers.asReadonly();
  readonly files = this.heldFiles.asReadonly();
  readonly user = this.heldUser.asReadonly();
  readonly path = this.heldPath.asReadonly();
  readonly busy = this.working.asReadonly();

  readonly choices = computed<PhxSelectOption<Id>[]>(() => {
    const state = this.users();
    return state.phase === 'loaded' ? state.users : [];
  });
  readonly crumbs = computed<PhxCrumb[]>(() =>
    crumbsOf(this.path(), this.t(KEYS.home)).map((crumb) =>
      crumb.path === ''
        ? { label: crumb.label, icon: 'folder', data: crumb.path }
        : { label: crumb.label, data: crumb.path },
    ),
  );
  readonly truncated = computed(() => {
    const state = this.files();
    return state.phase === 'loaded' && state.truncated;
  });
  readonly truncatedText = computed(() => this.t(KEYS.truncated, { count: LIST_LIMIT }));
  readonly usersMessage = computed(() => {
    const state = this.users();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  readonly filesMessage = computed(() => {
    const state = this.files();
    return state.phase === 'failed' ? describeError(this.i18n, state.error) : null;
  });
  readonly columns = computed<PhxColumn<FileEntry>[]>(() => [
    { key: 'name', label: this.t(KEYS.columnName), primary: true },
    {
      key: 'size',
      label: this.t(KEYS.columnSize),
      align: 'end',
      value: (row) => (row.kind === 'dir' ? '' : sizeOf(row.size)),
    },
    { key: 'mode', label: this.t(KEYS.columnMode) },
    {
      key: 'modified',
      label: this.t(KEYS.columnModified),
      type: 'date',
      value: (row) => row.modified ?? null,
    },
  ]);
  readonly rowActions = computed<PhxRowAction<FileEntry>[]>(() => [
    {
      id: 'open',
      label: this.t(KEYS.open),
      icon: 'folder',
      hidden: (row) => row.kind !== 'dir',
    },
    {
      id: 'download',
      label: this.t(KEYS.download),
      icon: 'download',
      hidden: (row) => row.kind !== 'file',
    },
    { id: 'rename', label: this.t(KEYS.rename), icon: 'pencil' },
    { id: 'chmod', label: this.t(KEYS.chmod), icon: 'lock' },
    { id: 'delete', label: this.t(KEYS.delete), icon: 'trash', appearance: 'danger' },
  ]);

  readonly fetch: PhxDataTableFetch<FileEntry> = (query) => {
    const state = this.files();
    const entries = state.phase === 'loaded' ? state.entries : [];
    return of(pageOf(entries, query));
  };

  constructor() {
    this.loadUsers();
    if (this.user() !== null) {
      this.load();
    }
  }

  entryOf(row: unknown): FileEntry {
    return row as FileEntry;
  }

  t(key: string, params?: Record<string, unknown>): string {
    return this.i18n.t(key, params);
  }

  loadUsers(): void {
    this.heldUsers.set({ phase: 'loading' });
    this.api.users().then(
      (users) => this.heldUsers.set({ phase: 'loaded', users }),
      (error: unknown) => this.heldUsers.set({ phase: 'failed', error: callErrorOf(error) }),
    );
  }

  load(): void {
    const user = this.user();
    if (user === null) {
      return;
    }
    const started = ++this.sequence;
    const path = this.path();
    this.heldFiles.set({ phase: 'loading' });
    this.api.list(user, path).then(
      (listing) => {
        if (started === this.sequence) {
          this.heldFiles.set({ phase: 'loaded', ...listing });
          this.reload.next();
        }
      },
      (error: unknown) => {
        if (started === this.sequence) {
          this.heldFiles.set({ phase: 'failed', error: callErrorOf(error) });
        }
      },
    );
  }

  pick(value: Id | Id[] | null): void {
    this.heldUser.set(typeof value === 'string' ? value : null);
    this.heldPath.set('');
    this.load();
  }

  go(select: PhxCrumbSelect): void {
    this.enter(typeof select.crumb.data === 'string' ? select.crumb.data : '');
  }

  open(event: PhxRowEvent<FileEntry>): void {
    if (event.row.kind === 'dir') {
      this.enter(joinPath(this.path(), event.row.name));
    }
  }

  act(event: PhxRowActionEvent<FileEntry>): void {
    const entry = event.row;
    switch (event.action.id) {
      case 'open':
        this.enter(joinPath(this.path(), entry.name));
        return;
      case 'download':
        void this.download(entry);
        return;
      case 'rename':
        void this.rename(entry);
        return;
      case 'chmod':
        void this.chmod(entry);
        return;
      case 'delete':
        void this.remove(entry);
        return;
    }
  }

  async mkdir(): Promise<void> {
    const name = await this.prompt(
      KEYS.mkdirTitle,
      KEYS.mkdirName,
      '',
      KEYS.invalidName,
      validName,
    );
    if (name !== null) {
      await this.run(FILE_MKDIR, KEYS.mkdir, { path: joinPath(this.path(), name) });
    }
  }

  async upload(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    const user = this.user();
    const picked = [...(input.files ?? [])];
    input.value = '';
    if (user === null || picked.length === 0) {
      return;
    }
    this.working.set(true);
    try {
      for (const file of picked) {
        try {
          await this.api.upload(user, joinPath(this.path(), file.name), file);
          this.toasts.success(this.t(KEYS.uploaded, { name: file.name }));
        } catch (error: unknown) {
          this.failed(error, this.t(KEYS.upload));
        }
      }
    } finally {
      this.working.set(false);
      this.load();
    }
  }

  private enter(path: string): void {
    this.heldPath.set(path);
    this.load();
  }

  private async download(entry: FileEntry): Promise<void> {
    const user = this.user();
    if (user !== null) {
      await this.downloads.save(
        { function: FILE_DOWNLOAD, params: { user, path: joinPath(this.path(), entry.name) } },
        this.t(KEYS.download),
      );
    }
  }

  private async rename(entry: FileEntry): Promise<void> {
    const name = await this.prompt(
      KEYS.renameTitle,
      KEYS.renameName,
      entry.name,
      KEYS.invalidName,
      validName,
      { name: entry.name },
    );
    if (name !== null && name !== entry.name) {
      await this.run(FILE_RENAME, KEYS.rename, {
        from: joinPath(this.path(), entry.name),
        to: joinPath(this.path(), name),
      });
    }
  }

  private async chmod(entry: FileEntry): Promise<void> {
    const mode = await this.prompt(
      KEYS.chmodTitle,
      KEYS.chmodMode,
      entry.mode.replace(/^0(?=[0-7]{3}$)/, ''),
      KEYS.invalidMode,
      validMode,
      { name: entry.name },
    );
    if (mode !== null) {
      await this.run(FILE_CHMOD, KEYS.chmod, { path: joinPath(this.path(), entry.name), mode });
    }
  }

  private async remove(entry: FileEntry): Promise<void> {
    const confirmed = await this.confirms.confirm({
      title: this.t(KEYS.deleteTitle, { name: entry.name }),
      text: this.t(KEYS.deleteText, { name: entry.name }),
      confirmLabel: this.t(KEYS.delete),
      cancelLabel: this.t(KEYS.cancel),
      destructive: true,
    });
    if (confirmed) {
      await this.run(FILE_DELETE, KEYS.delete, { path: joinPath(this.path(), entry.name) });
    }
  }

  private async run(func: string, title: string, params: Record<string, unknown>): Promise<void> {
    const user = this.user();
    if (user === null) {
      return;
    }
    this.working.set(true);
    try {
      await this.api.change(func, { user, ...params });
    } catch (error: unknown) {
      this.failed(error, this.t(title));
    } finally {
      this.working.set(false);
      this.load();
    }
  }

  private failed(error: unknown, title: string): void {
    this.toasts.danger(describeError(this.i18n, callErrorOf(error)), { title });
  }

  private async prompt(
    title: string,
    label: string,
    value: string,
    invalid: string,
    valid: (value: string) => boolean,
    params?: Record<string, unknown>,
  ): Promise<string | null> {
    const data: PromptData = {
      label: this.t(label),
      value,
      submit: this.t(KEYS.save),
      cancel: this.t(KEYS.cancel),
      invalid: this.t(invalid),
      valid,
    };
    const ref = this.modals.open<FilePromptComponent, PromptData, string>(FilePromptComponent, {
      title: this.t(title, params),
      width: PROMPT_WIDTH,
      data,
    });
    const result = await firstValueFrom(ref.afterClosed, { defaultValue: undefined });
    return result ?? null;
  }
}
