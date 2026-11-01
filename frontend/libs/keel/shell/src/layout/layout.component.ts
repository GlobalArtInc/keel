import { Component, computed, inject, signal } from '@angular/core';
import { Router, RouterOutlet } from '@angular/router';
import { KeelBoot, KeelI18n, KeelProblems } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { BootBrand } from '@globalart/keel/meta';
import { describeError } from '@globalart/keel/renderer';
import {
  PhxBannerComponent,
  PhxBrandLogoComponent,
  PhxIconButtonComponent,
  PhxLayoutComponent,
  PhxSidebarComponent,
  PhxSidebarGroupComponent,
  PhxSidebarItemComponent,
  PhxTabBarComponent,
  PhxTextButtonComponent,
  PhxToastService,
} from '@globalart/phoenix-ui';
import { KeelRailComponent } from '../rail/rail.component';
import { KeelSu } from '../rail/su';
import { KeelWorkspace } from '../workspace/workspace';
import { brandOf, sidebarOf, type MenuEntry } from './sidebar';

export const MENU_KEY = 'menu.system';
export const ACTING_AS_KEY = 'keel.shell.acting_as.banner';
export const ACTING_EXIT_KEY = 'keel.shell.acting_as.exit';
export const SIDEBAR_STORAGE_KEY = 'keel-sidebar-collapsed';
export const TABS_LABEL_KEY = 'keel.shell.workspace.tabs';
export const REFRESH_LABEL_KEY = 'keel.shell.workspace.refresh';

const NO_BRAND: BootBrand = { name: '', hue: 0, chroma: 0 };

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
  selector: 'keel-layout',
  imports: [
    RouterOutlet,
    PhxLayoutComponent,
    PhxSidebarComponent,
    PhxSidebarGroupComponent,
    PhxSidebarItemComponent,
    PhxBrandLogoComponent,
    PhxBannerComponent,
    PhxIconButtonComponent,
    PhxTabBarComponent,
    PhxTextButtonComponent,
    KeelRailComponent,
  ],
  template: `
    <phx-layout [storageKey]="storageKey">
      <phx-sidebar phxSidebar [label]="menuLabel()">
        <phx-brand-logo [src]="brand().logo" [alt]="brand().name">
          @if (brand().logo === null) {
            <span phxLogoFull class="brand">{{ brand().name }}</span>
          }
          <span phxLogoMark class="mark" aria-hidden="true">{{ brand().mark }}</span>
        </phx-brand-logo>
        @for (section of sections(); track section.product) {
          <phx-sidebar-group
            class="section"
            [label]="section.label"
            [attr.data-product]="section.product"
          >
            @for (group of section.groups; track group.id) {
              <phx-sidebar-group class="group" [label]="group.label" [attr.data-group]="group.id">
                @for (entry of group.items; track entry.func) {
                  <phx-sidebar-item
                    [label]="entry.label"
                    [icon]="entry.icon"
                    [href]="entry.url"
                    [badge]="problems.badge(entry.problem)"
                    [active]="isActive(entry)"
                    [attr.data-func]="entry.func"
                    (click)="open($event, entry)"
                  />
                }
              </phx-sidebar-group>
            }
          </phx-sidebar-group>
        }
      </phx-sidebar>
      <keel-rail phxRail />
      <phx-tab-bar
        phxTabBar
        [label]="tabsLabel()"
        (activate)="workspace.activate($event)"
        (close)="workspace.close($event)"
        (closeOthers)="workspace.closeOthers($event)"
        (closeAll)="workspace.closeAll()"
      >
        <button
          phxTabBarStart
          phxIconButton
          type="button"
          icon="rotate-cw"
          [label]="refreshLabel()"
          [title]="refreshLabel()"
          (click)="workspace.refresh()"
        ></button>
      </phx-tab-bar>
      @if (actor()) {
        <phx-banner class="acting" status="warning" role="status" data-acting-as>
          {{ actingText() }}
          <button
            phxBannerActions
            phxTextButton="link"
            type="button"
            class="exit"
            [disabled]="leaving()"
            (click)="exit()"
          >
            {{ exitLabel() }}
          </button>
        </phx-banner>
      }
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
    .brand {
      font: var(--phx-heading-18);
      color: var(--phx-ink);
      white-space: nowrap;
    }
    .mark {
      display: inline-grid;
      place-items: center;
      width: 28px;
      height: 28px;
      font: var(--phx-heading-14);
      color: var(--phx-ink-on-status);
      background-color: var(--phx-accent);
      border-radius: var(--phx-radius-medium);
    }
    .group:last-child {
      padding-bottom: 0;
    }
    .acting {
      margin: 0 0 16px;
    }
  `,
})
export class KeelLayoutComponent {
  protected readonly problems = inject(KeelProblems);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);
  private readonly su = inject(KeelSu);
  private readonly toasts = inject(PhxToastService);
  private readonly busy = signal(false);

  protected readonly workspace = inject(KeelWorkspace);
  protected readonly storageKey = SIDEBAR_STORAGE_KEY;
  readonly leaving = this.busy.asReadonly();
  readonly sections = computed(() => {
    const payload = this.boot.payload();
    return payload === null ? [] : sidebarOf(payload, (key) => this.i18n.t(key));
  });
  readonly brand = computed(() =>
    brandOf(this.boot.brand() ?? NO_BRAND, (key) => this.i18n.t(key)),
  );
  readonly actor = computed(() => this.boot.principal()?.actor ?? null);

  protected readonly menuLabel = computed(() => this.i18n.t(MENU_KEY));
  protected readonly actingText = computed(() =>
    this.i18n.t(ACTING_AS_KEY, { login: this.boot.principal()?.login ?? '' }),
  );
  protected readonly exitLabel = computed(() => this.i18n.t(ACTING_EXIT_KEY));
  protected readonly tabsLabel = computed(() => this.i18n.t(TABS_LABEL_KEY));
  protected readonly refreshLabel = computed(() => this.i18n.t(REFRESH_LABEL_KEY));

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
    void this.workspace.openFunc(entry);
  }

  async exit(): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.busy.set(true);
    try {
      const envelope = await this.su.exit();
      if (!envelope.ok) {
        this.toasts.danger(describeError(this.i18n, envelope.error));
      }
    } catch (error: unknown) {
      this.toasts.danger(describeError(this.i18n, unreachable(error)));
    } finally {
      this.busy.set(false);
    }
  }
}
