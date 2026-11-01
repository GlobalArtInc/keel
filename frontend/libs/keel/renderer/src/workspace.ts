import { InjectionToken } from '@angular/core';
import type { PhxColumnFilter, PhxSort } from '@globalart/phoenix-ui';
import type { Observable } from 'rxjs';

export interface WorkspaceListState {
  search: string;
  sort: PhxSort | null;
  filters: PhxColumnFilter[];
  page: number;
}

export interface KeelWorkspaceHost {
  activeId(): string | null;
  visible(): boolean;
  expectChild(openerId: string | null, label?: string): void;
  retitle(id: string, title: string): void;
  readonly refreshes: Observable<void>;
  listState(id: string): WorkspaceListState | null;
  saveListState(id: string, state: WorkspaceListState): void;
}

export const KEEL_WORKSPACE = new InjectionToken<KeelWorkspaceHost>('KEEL_WORKSPACE');
