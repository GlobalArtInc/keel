import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  untracked,
} from '@angular/core';
import { Router } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import { unreachable } from '@globalart/keel/forms';
import type { Envelope } from '@globalart/keel/meta';
import { describeError } from '@globalart/keel/renderer';
import {
  PhxDropdownComponent,
  PhxModalService,
  PhxRailComponent,
  PhxRailItemComponent,
  PhxThemeService,
  PhxToastService,
  PhxUserMenuComponent,
  type PhxThemeState,
  type PhxUserMenuItem,
  type PhxUserMenuLanguage,
} from '@globalart/phoenix-ui';
import { firstValueFrom } from 'rxjs';
import { LOGOUT_PATH, screenUrl } from '../guard';
import { levelLabel, type Translate } from '../layout/sidebar';
import { TOTP_ENROLL } from '../security/totp';
import { ACT_AS_TITLE_KEY, KeelActAsComponent } from './act-as.component';
import { KeelSu } from './su';

export const PREF_SET = 'settings.pref.set';
export const LANG_PREF = 'lang';
export const THEME_PREF = 'theme';
export const LANGUAGES: readonly string[] = ['en', 'ru'];
export const LANGUAGE_PREFIX = 'keel.shell.language';
export const RAIL_NOTIFICATIONS_KEY = 'keel.shell.rail.notifications';
export const RAIL_TASKS_KEY = 'keel.shell.rail.tasks';
export const RAIL_HELP_KEY = 'keel.shell.rail.help';
export const RAIL_LATER_KEY = 'keel.shell.rail.later';
export const USER_LANGUAGE_KEY = 'keel.shell.user.language';
export const USER_THEME_KEY = 'keel.shell.user.theme';
export const USER_ACT_AS_KEY = 'keel.shell.user.act_as';
export const USER_SESSIONS_KEY = 'keel.shell.user.sessions';
export const USER_TOKENS_KEY = 'keel.shell.user.tokens';
export const USER_SECURITY_KEY = 'keel.shell.user.security';
export const USER_LOGOUT_KEY = 'keel.shell.user.logout';
export const SESSIONS_FUNCTION = 'auth.session.list';
export const TOKENS_FUNCTION = 'auth.token.list';
export const SECURITY_FUNCTION = TOTP_ENROLL;

export const ACT_AS_ENTRY = 'act_as';
export const LOGOUT_ENTRY = 'logout';
export const USER_SCREENS: Readonly<Record<string, string>> = {
  sessions: SESSIONS_FUNCTION,
  tokens: TOKENS_FUNCTION,
  security: SECURITY_FUNCTION,
};

export function userMenuItems(label: Translate, actable: boolean): PhxUserMenuItem[] {
  const items: PhxUserMenuItem[] = [];
  if (actable) {
    items.push({ id: ACT_AS_ENTRY, label: label(USER_ACT_AS_KEY), icon: 'user-cog' });
  }
  items.push(
    { id: 'sessions', label: label(USER_SESSIONS_KEY), icon: 'monitor' },
    { id: 'tokens', label: label(USER_TOKENS_KEY), icon: 'key' },
    { id: 'security', label: label(USER_SECURITY_KEY), icon: 'shield' },
    { id: LOGOUT_ENTRY, label: label(USER_LOGOUT_KEY), icon: 'log-out' },
  );
  return items;
}

export function languageOptions(label: Translate): PhxUserMenuLanguage[] {
  return LANGUAGES.map((value) => ({ value, label: label(`${LANGUAGE_PREFIX}.${value}`) }));
}

@Component({
  selector: 'keel-rail',
  imports: [PhxRailComponent, PhxRailItemComponent, PhxDropdownComponent, PhxUserMenuComponent],
  template: `
    <phx-rail>
      <phx-rail-item
        icon="user"
        appearance="filled"
        data-rail="user"
        [label]="login()"
        [dropdown]="menu"
      />
      <phx-dropdown #menu mobileSheet side="left" align="start" [offset]="8" [label]="login()">
        <phx-user-menu
          [name]="login()"
          [role]="level()"
          [items]="items()"
          [languages]="languages()"
          [language]="language()"
          [languageLabel]="languageLabel()"
          [themeLabel]="themeLabel()"
          [themeColors]="false"
          (languageChange)="switchLanguage($event)"
          (themeChange)="switchTheme($event)"
          (sectionToggle)="menu.updatePosition()"
          (select)="choose($event.id); menu.select()"
        />
      </phx-dropdown>
      <phx-rail-item
        icon="bell"
        data-rail="notifications"
        disabled
        [label]="later(notificationsKey)"
      />
      <phx-rail-item icon="list-checks" data-rail="tasks" disabled [label]="later(tasksKey)" />
      <phx-rail-item
        icon="circle-question-mark"
        data-rail="help"
        disabled
        [label]="later(helpKey)"
      />
    </phx-rail>
  `,
  styles: `
    :host {
      display: block;
      height: 100%;
      min-height: 0;
    }
  `,
  host: {
    '[attr.data-actable]': 'su.available()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelRailComponent {
  protected readonly su = inject(KeelSu);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly client = inject(KeelClient);
  private readonly theme = inject(PhxThemeService);
  private readonly toasts = inject(PhxToastService);
  private readonly modals = inject(PhxModalService);
  private readonly router = inject(Router);

  protected readonly notificationsKey = RAIL_NOTIFICATIONS_KEY;
  protected readonly tasksKey = RAIL_TASKS_KEY;
  protected readonly helpKey = RAIL_HELP_KEY;

  readonly login = computed(() => this.boot.principal()?.login ?? '');
  readonly level = computed(() => {
    const principal = this.boot.principal();
    if (principal === null) {
      return null;
    }
    return levelLabel(this.boot.products(), principal.level, this.i18n.messages(), (key) =>
      this.i18n.t(key),
    );
  });
  readonly items = computed(() => userMenuItems((key) => this.i18n.t(key), this.su.available()));
  readonly languages = computed(() => languageOptions((key) => this.i18n.t(key)));
  readonly language = computed(() => this.i18n.lang());

  protected readonly languageLabel = computed(() => this.i18n.t(USER_LANGUAGE_KEY));
  protected readonly themeLabel = computed(() => this.i18n.t(USER_THEME_KEY));

  constructor() {
    effect(() => {
      this.boot.principal();
      untracked(() => void this.su.probe());
    });
  }

  later(key: string): string {
    return `${this.i18n.t(key)} (${this.i18n.t(RAIL_LATER_KEY)})`;
  }

  async switchLanguage(lang: string | null): Promise<void> {
    if (lang === null || lang === this.i18n.lang()) {
      return;
    }
    const envelope = await this.pref(LANG_PREF, lang);
    if (envelope.ok) {
      await this.i18n.load(lang);
    }
  }

  async switchTheme(state: PhxThemeState): Promise<void> {
    this.theme.setScheme(state.scheme);
    await this.pref(THEME_PREF, state.scheme);
  }

  choose(id: string): void {
    if (id === ACT_AS_ENTRY) {
      this.actAs();
      return;
    }
    if (id === LOGOUT_ENTRY) {
      void this.router.navigateByUrl(`/${LOGOUT_PATH}`);
      return;
    }
    const func = USER_SCREENS[id];
    if (func !== undefined) {
      void this.router.navigateByUrl(screenUrl(this.router, func));
    }
  }

  actAs(): void {
    this.modals.open(KeelActAsComponent, { title: this.i18n.t(ACT_AS_TITLE_KEY), width: 400 });
  }

  private async pref(key: string, value: string): Promise<Envelope> {
    const envelope = await firstValueFrom(this.client.call(PREF_SET, { key, value })).catch(
      (error: unknown): Envelope => ({ ok: false, error: unreachable(error) }),
    );
    if (!envelope.ok) {
      this.toasts.danger(describeError(this.i18n, envelope.error));
    }
    return envelope;
  }
}
