import { Injectable, inject } from '@angular/core';
import { Router } from '@angular/router';
import { PhxTabsService, type PhxTab } from '@globalart/phoenix-ui';

export const SCREEN_PATH = 'f';

export interface ScreenTarget {
  screen: string;
  params: Record<string, unknown>;
}

export function objectTab(url: string, target: ScreenTarget, label: string): PhxTab {
  return { id: url, label, data: { func: target.screen, params: target.params } };
}

@Injectable({ providedIn: 'root' })
export class KeelNavigation {
  private readonly router = inject(Router);
  private readonly tabs = inject(PhxTabsService);

  url(target: ScreenTarget): string {
    const tree = this.router.createUrlTree([`/${SCREEN_PATH}`, target.screen], {
      queryParams: target.params,
    });
    return this.router.serializeUrl(tree);
  }

  screen(target: ScreenTarget): Promise<boolean> {
    return this.router.navigateByUrl(this.url(target));
  }

  object(target: ScreenTarget, label: string): Promise<boolean> {
    const url = this.url(target);
    this.tabs.open(objectTab(url, target, label));
    return this.router.navigateByUrl(url);
  }
}
