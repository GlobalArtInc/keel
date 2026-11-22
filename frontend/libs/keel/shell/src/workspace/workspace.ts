import { Injectable, computed, effect, inject, signal, untracked } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  NavigationCancel,
  NavigationEnd,
  NavigationError,
  NavigationSkipped,
  PRIMARY_OUTLET,
  Router,
  type Event as RouterEvent,
  type Params,
} from '@angular/router';
import { KeelBoot, KeelI18n } from '@globalart/keel/client';
import {
  SCREEN_PATH,
  type KeelWorkspaceHost,
  type WorkspaceListState,
} from '@globalart/keel/renderer';
import {
  PhxTabsService,
  type PhxColumnFilter,
  type PhxIconName,
  type PhxSort,
  type PhxTab,
} from '@globalart/phoenix-ui';
import { Subject, filter, fromEvent, type Observable } from 'rxjs';
import { HOME_URL, hasDashboard } from '../guard';
import { menuEntries, screenFunc, sectionOf, type MenuEntry } from '../layout/sidebar';

const WORKSPACE_VERSION = 1;
const WORKSPACE_LIMIT = 20;
const HOME_TAB_ID = 'home';
const HOME_TITLE_KEY = 'keel.shell.home.title';
const WORKSPACE_STORAGE_PREFIX = 'keel.workspace.v2.';
const HOME_ICON: PhxIconName = 'layout-dashboard';
const SELECTED_PARAM = 'selected';
const MOBILE_MAX_WIDTH = 640;

export interface WorkspaceTabData {
  url: string;
  func: string | null;
  lastActive: number;
}

export interface WorkspaceTab extends PhxTab {
  data: WorkspaceTabData;
}

export interface WorkspaceState {
  v: 1;
  tabs: WorkspaceTab[];
  active: string | null;
  lists: Record<string, WorkspaceListState>;
}

interface PendingChild {
  parent: string | null;
  label: string | null;
}

interface Place {
  id: string;
  func: string | null;
}

function emptyState(): WorkspaceState {
  return { v: WORKSPACE_VERSION, tabs: [], active: null, lists: {} };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isSort(value: unknown): value is PhxSort {
  return (
    isRecord(value) &&
    typeof value['key'] === 'string' &&
    (value['dir'] === 'asc' || value['dir'] === 'desc')
  );
}

function isFilter(value: unknown): value is PhxColumnFilter {
  const filterValue = isRecord(value) ? value['value'] : undefined;
  return (
    isRecord(value) &&
    typeof value['key'] === 'string' &&
    (typeof filterValue === 'string' ||
      (Array.isArray(filterValue) && filterValue.every((item) => typeof item === 'string')))
  );
}

function isListState(value: unknown): value is WorkspaceListState {
  return (
    isRecord(value) &&
    typeof value['search'] === 'string' &&
    typeof value['page'] === 'number' &&
    (value['sort'] === null || isSort(value['sort'])) &&
    Array.isArray(value['filters']) &&
    value['filters'].every(isFilter)
  );
}

function isTabData(value: unknown): value is WorkspaceTabData {
  return (
    isRecord(value) &&
    typeof value['url'] === 'string' &&
    (value['func'] === null || typeof value['func'] === 'string') &&
    typeof value['lastActive'] === 'number'
  );
}

function isWorkspaceTab(value: unknown): value is WorkspaceTab {
  return (
    isRecord(value) &&
    typeof value['id'] === 'string' &&
    typeof value['label'] === 'string' &&
    (value['pinned'] === undefined || typeof value['pinned'] === 'boolean') &&
    (value['parent'] === undefined || typeof value['parent'] === 'string') &&
    isTabData(value['data'])
  );
}

function isState(value: unknown): value is WorkspaceState {
  return (
    isRecord(value) &&
    value['v'] === WORKSPACE_VERSION &&
    Array.isArray(value['tabs']) &&
    value['tabs'].every(isWorkspaceTab) &&
    (value['active'] === null || typeof value['active'] === 'string') &&
    isRecord(value['lists']) &&
    Object.values(value['lists']).every(isListState)
  );
}

function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function parseState(raw: string | null): WorkspaceState {
  const value = raw === null ? null : parseJson(raw);
  return isState(value) ? value : emptyState();
}

function readState(key: string): WorkspaceState {
  try {
    return parseState(localStorage.getItem(key));
  } catch {
    return emptyState();
  }
}

function attempt(action: () => void): void {
  try {
    action();
  } catch {
    return;
  }
}

function listKey(tab: string, func: string): string {
  return `${tab}|${func}`;
}

function tabOfListKey(key: string): string {
  return key.slice(0, key.lastIndexOf('|'));
}

function tabData(url: string, func: string | null, lastActive: number): WorkspaceTabData {
  return { url, func, lastActive };
}

function dataOf(tab: PhxTab): WorkspaceTabData | null {
  return isTabData(tab.data) ? tab.data : null;
}

function urlOf(tab: PhxTab): string | null {
  return dataOf(tab)?.url ?? null;
}

function funcOf(tab: PhxTab): string | null {
  return dataOf(tab)?.func ?? null;
}

function lastActiveOf(tab: PhxTab): number {
  return dataOf(tab)?.lastActive ?? 0;
}

@Injectable({ providedIn: 'root' })
export class KeelWorkspace implements KeelWorkspaceHost {
  private readonly router = inject(Router);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly tabs = inject(PhxTabsService);
  private readonly lists = signal<Record<string, WorkspaceListState>>({});
  private readonly width = signal(window.innerWidth);
  private readonly refreshed = new Subject<void>();
  private readonly storageKey = computed(() => {
    const principal = this.boot.principal();
    return principal === null ? null : `${WORKSPACE_STORAGE_PREFIX}${principal.id}`;
  });
  private loaded: string | null = null;
  private pending = false;
  private expected: PendingChild | null = null;

  readonly refreshes: Observable<void> = this.refreshed.asObservable();

  constructor() {
    effect(() => {
      const key = this.storageKey();
      untracked(() => this.hydrate(key));
    });
    effect(() => {
      this.tabs.tabs();
      this.tabs.activeId();
      this.lists();
      untracked(() => this.persist());
    });
    this.router.events
      .pipe(
        filter((event): event is NavigationEnd => event instanceof NavigationEnd),
        takeUntilDestroyed(),
      )
      .subscribe((event) => this.arrive(event.urlAfterRedirects, this.takeExpectation()));
    this.router.events.pipe(filter(isAbandoned), takeUntilDestroyed()).subscribe(() => {
      this.expected = null;
    });
    fromEvent(window, 'resize')
      .pipe(takeUntilDestroyed())
      .subscribe(() => this.width.set(window.innerWidth));
  }

  openFunc(entry: MenuEntry): Promise<boolean> {
    const funcs = [entry.func, ...entry.tabs.map((tab) => tab.func)];
    const open = this.tabs.tabs().find((tab) => {
      const func = funcOf(tab);
      return !tab.parent && func !== null && funcs.includes(func);
    });
    return this.router.navigateByUrl(open === undefined ? entry.url : (urlOf(open) ?? entry.url));
  }

  activate(tab: PhxTab): void {
    const url = urlOf(tab);
    if (url !== null) {
      void this.router.navigateByUrl(url);
    }
  }

  close(tab: PhxTab): void {
    if (tab.pinned) {
      return;
    }
    const routed = this.routedId();
    this.tabs.close(tab.id);
    this.prune();
    if (routed === tab.id) {
      this.leave();
    }
  }

  closeOthers(tab: PhxTab): void {
    this.tabs.closeOthers(tab.id);
    this.prune();
    this.activate(tab);
  }

  closeAll(): void {
    this.tabs.closeAll();
    this.prune();
    this.leave();
  }

  refresh(): void {
    this.refreshed.next();
  }

  retitle(id: string, title: string): void {
    this.tabs.update(id, { label: title });
  }

  activeId(): string | null {
    return this.tabs.activeId();
  }

  visible(): boolean {
    return this.tabs.tabs().length > 0 && this.width() > MOBILE_MAX_WIDTH;
  }

  expectChild(openerId: string | null, label?: string): void {
    const opener =
      openerId === null ? undefined : this.tabs.tabs().find((tab) => tab.id === openerId);
    this.expected = { parent: opener?.label ?? null, label: label ?? null };
  }

  listState(tab: string, func: string): WorkspaceListState | null {
    return this.lists()[listKey(tab, func)] ?? null;
  }

  saveListState(tab: string, func: string, state: WorkspaceListState): void {
    this.lists.update((lists) => ({ ...lists, [listKey(tab, func)]: state }));
  }

  private takeExpectation(): PendingChild | null {
    const child = this.expected;
    this.expected = null;
    return child;
  }

  private arrive(url: string, child: PendingChild | null): void {
    const place = this.locate(url);
    if (place === null) {
      return;
    }
    const now = Date.now();
    if (place.func === null) {
      if (hasDashboard(this.boot.payload())) {
        this.show(
          { id: HOME_TAB_ID, label: this.i18n.t(HOME_TITLE_KEY), icon: HOME_ICON, pinned: true },
          tabData(HOME_URL, null, now),
        );
      }
      return;
    }
    const parent = child?.parent ?? null;
    this.show(
      {
        id: place.id,
        label: child?.label ?? this.labelOf(place.func),
        ...(parent === null ? {} : { parent }),
      },
      tabData(url, place.func, now),
    );
  }

  private show(candidate: PhxTab, data: WorkspaceTabData): void {
    if (this.tabs.has(candidate.id)) {
      this.tabs.update(candidate.id, { data });
    } else {
      if (!candidate.pinned) {
        this.makeRoom();
      }
      this.tabs.open({ ...candidate, data }, false);
    }
    this.tabs.activate(candidate.id);
  }

  private makeRoom(): void {
    const unpinned = this.tabs.tabs().filter((tab) => !tab.pinned);
    if (unpinned.length < WORKSPACE_LIMIT) {
      return;
    }
    const [victim] = unpinned
      .filter((tab) => tab.id !== this.tabs.activeId())
      .sort((first, second) => lastActiveOf(first) - lastActiveOf(second));
    if (victim === undefined) {
      return;
    }
    this.tabs.close(victim.id);
    this.prune();
  }

  private leave(): void {
    const next = this.tabs.activeTab();
    const url = next === null ? null : urlOf(next);
    void this.router.navigateByUrl(url ?? HOME_URL);
  }

  private routedId(): string | null {
    return this.locate(this.router.url)?.id ?? null;
  }

  private locate(url: string): Place | null {
    const tree = this.router.parseUrl(url);
    const segments = tree.root.children[PRIMARY_OUTLET]?.segments ?? [];
    if (segments.length === 0) {
      return { id: HOME_TAB_ID, func: null };
    }
    const func = screenFunc(tree);
    if (func === null) {
      return null;
    }
    const section = sectionOf(this.boot.menu(), func);
    const id =
      section === null
        ? this.tabId([`/${SCREEN_PATH}`, func], tree.queryParams)
        : `/${section.path}`;
    return { id, func };
  }

  private tabId(commands: string[], params: Params): string {
    const kept: Params = Object.fromEntries(
      Object.keys(params)
        .filter((key) => key !== SELECTED_PARAM)
        .sort()
        .map((key) => [key, params[key]]),
    );
    return this.router.serializeUrl(this.router.createUrlTree(commands, { queryParams: kept }));
  }

  private labelOf(func: string): string {
    const section = sectionOf(this.boot.menu(), func);
    if (section !== null) {
      return this.i18n.t(section.label);
    }
    const items = this.boot.menu().flatMap((group) => group.items);
    const entry = menuEntries(items, (key) => this.i18n.t(key)).find((item) => item.func === func);
    return entry?.label ?? func;
  }

  private prune(): void {
    const ids = new Set(this.tabs.tabs().map((tab) => tab.id));
    this.lists.update((lists) =>
      Object.fromEntries(Object.entries(lists).filter(([key]) => ids.has(tabOfListKey(key)))),
    );
  }

  private hydrate(key: string | null): void {
    if (key === this.loaded) {
      return;
    }
    this.flush();
    this.loaded = key;
    const state = key === null ? emptyState() : readState(key);
    this.tabs.tabs.set(state.tabs);
    this.tabs.activeId.set(null);
    this.lists.set(state.lists);
    this.arrive(this.router.url, null);
  }

  private persist(): void {
    if (this.pending) {
      return;
    }
    this.pending = true;
    queueMicrotask(() => this.flush());
  }

  private flush(): void {
    if (!this.pending) {
      return;
    }
    this.pending = false;
    this.write();
  }

  private write(): void {
    const key = this.loaded;
    if (key === null) {
      return;
    }
    const state: WorkspaceState = {
      v: WORKSPACE_VERSION,
      tabs: this.tabs.tabs().filter(isWorkspaceTab),
      active: this.tabs.activeId(),
      lists: this.lists(),
    };
    attempt(() => localStorage.setItem(key, JSON.stringify(state)));
  }
}

function isAbandoned(event: RouterEvent): boolean {
  return (
    event instanceof NavigationCancel ||
    event instanceof NavigationError ||
    event instanceof NavigationSkipped
  );
}
