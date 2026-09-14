import { Location } from '@angular/common';
import { Component, computed, inject } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { Router, RouterOutlet } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { isIconName } from '@globalart/keel/forms';
import type { BootMenuItem } from '@globalart/keel/meta';
import { SCREEN_PATH } from '@globalart/keel/renderer';
import {
  PHX_LABELS,
  PhxErrorComponent,
  PhxFullscreenLayoutComponent,
  PhxLayoutComponent,
  PhxLoaderComponent,
  PhxSidebarItemComponent,
  type PhxIconName,
} from '@globalart/phoenix-ui';
import { HOME_URL, KeelStartup, authRoute, loginUrl } from './guard';

export const BOOT_FAILED_KEY = 'keel.shell.boot_failed';
export const MENU_KEY = 'menu.system';

export interface MenuEntry {
  func: string;
  url: string;
  label: string;
  icon: PhxIconName | null;
}

export function menuEntries(
  items: readonly BootMenuItem[],
  label: (key: string) => string,
): MenuEntry[] {
  return items.map((item): MenuEntry => ({
    func: item.func,
    url: `/${SCREEN_PATH}/${item.func}`,
    label: label(item.label),
    icon: isIconName(item.icon) ? item.icon : null,
  }));
}

export function plainClick(event: MouseEvent): boolean {
  return (
    !event.defaultPrevented &&
    event.button === 0 &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.shiftKey &&
    !event.altKey
  );
}

@Component({
  selector: 'keel-shell',
  imports: [RouterOutlet, PhxFullscreenLayoutComponent, PhxLoaderComponent, PhxErrorComponent],
  template: `
    @switch (startup.state()) {
      @case ('ready') {
        <router-outlet />
      }
      @case ('failed') {
        <phx-fullscreen-layout>
          <phx-error [text]="failedText()" [retryLabel]="labels.retry" (retry)="retry()" />
        </phx-fullscreen-layout>
      }
      @default {
        <phx-fullscreen-layout>
          <div class="loading" role="status">
            <phx-loader [size]="24" [label]="labels.loading" />
          </div>
        </phx-fullscreen-layout>
      }
    }
  `,
  styles: `
    :host {
      display: block;
      min-height: 100%;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0 40px;
    }
    phx-error {
      margin-bottom: 40px;
    }
  `,
  host: {
    '[attr.data-startup]': 'startup.state()',
  },
})
export class KeelShellComponent {
  protected readonly startup = inject(KeelStartup);
  protected readonly labels = inject(PHX_LABELS);
  private readonly client = inject(KeelClient);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);
  private readonly location = inject(Location);

  protected readonly failedText = computed(() => this.i18n.t(BOOT_FAILED_KEY));

  constructor() {
    this.client.unauthenticated.pipe(takeUntilDestroyed()).subscribe(() => this.expire());
  }

  retry(): void {
    this.startup.reset();
    void this.router
      .navigateByUrl(this.location.path() || HOME_URL, { onSameUrlNavigation: 'reload' })
      .catch(() => false);
  }

  private expire(): void {
    const current = this.router.url;
    if (authRoute(current)) {
      return;
    }
    this.startup.reset();
    void this.router.navigateByUrl(loginUrl(this.router, current));
  }
}

@Component({
  selector: 'keel-layout',
  imports: [RouterOutlet, PhxLayoutComponent, PhxSidebarItemComponent],
  template: `
    <phx-layout>
      <nav phxSidebar class="menu" [attr.aria-label]="menuLabel()">
        @for (entry of entries(); track entry.func) {
          <phx-sidebar-item
            [label]="entry.label"
            [icon]="entry.icon"
            [href]="entry.url"
            [active]="isActive(entry)"
            (click)="open($event, entry)"
          />
        }
      </nav>
      <router-outlet />
    </phx-layout>
  `,
  styles: `
    :host {
      display: block;
      height: 100vh;
    }
    @supports (height: 100dvh) {
      :host {
        height: 100dvh;
      }
    }
    .menu {
      display: flex;
      flex-direction: column;
      gap: 2px;
      height: 100%;
      padding: 16px 8px;
      overflow: auto;
    }
  `,
})
export class KeelLayoutComponent {
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);

  protected readonly entries = computed(() =>
    menuEntries(
      this.boot.menu().flatMap((group) => group.items),
      (key) => this.i18n.t(key),
    ),
  );
  protected readonly menuLabel = computed(() => this.i18n.t(MENU_KEY));

  isActive(entry: MenuEntry): boolean {
    return this.router.isActive(entry.url, {
      paths: 'exact',
      queryParams: 'ignored',
      fragment: 'ignored',
      matrixParams: 'ignored',
    });
  }

  open(event: MouseEvent, entry: MenuEntry): void {
    if (!plainClick(event)) {
      return;
    }
    event.preventDefault();
    void this.router.navigateByUrl(entry.url);
  }
}
