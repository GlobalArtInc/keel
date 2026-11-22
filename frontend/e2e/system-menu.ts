import { expect } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

export type SystemView = 'list' | 'tree' | 'stream' | 'object';

export interface SystemScreen {
  group: string;
  section: string;
  tab?: string;
  title: string;
  func: string;
  view: SystemView;
}

export const SYSTEM_SCREENS: readonly SystemScreen[] = [
  {
    group: 'access',
    section: 'Users',
    tab: 'Accounts',
    title: 'Accounts',
    func: 'keel.account.list',
    view: 'tree',
  },
  {
    group: 'access',
    section: 'Users',
    tab: 'Users',
    title: 'Principals',
    func: 'keel.principal.list',
    view: 'list',
  },
  {
    group: 'access',
    section: 'Users',
    tab: 'API tokens',
    title: 'API tokens',
    func: 'auth.token.list',
    view: 'list',
  },
  {
    group: 'access',
    section: 'Users',
    tab: 'Sessions',
    title: 'Sessions',
    func: 'auth.session.list',
    view: 'list',
  },
  {
    group: 'access',
    section: 'Users',
    tab: 'IP rules',
    title: 'IP rules',
    func: 'auth.ip_rule.list',
    view: 'list',
  },
  {
    group: 'access',
    section: 'Users',
    tab: 'Permission grants',
    title: 'Permission grants',
    func: 'auth.grant.list',
    view: 'list',
  },
  {
    group: 'monitor',
    section: 'Problems',
    title: 'Problems',
    func: 'notify.problem.list',
    view: 'list',
  },
  {
    group: 'monitor',
    section: 'Tasks',
    tab: 'Queue',
    title: 'Tasks',
    func: 'task.list',
    view: 'list',
  },
  {
    group: 'monitor',
    section: 'Tasks',
    tab: 'Schedule',
    title: 'Scheduled jobs',
    func: 'cron.job.list',
    view: 'list',
  },
  {
    group: 'monitor',
    section: 'Logs',
    tab: 'Audit trail',
    title: 'Audit trail',
    func: 'audit.list',
    view: 'list',
  },
  {
    group: 'monitor',
    section: 'Logs',
    tab: 'Server log',
    title: 'Log',
    func: 'keel.logs.stream',
    view: 'stream',
  },
  { group: 'monitor', section: 'Health', title: 'Health', func: 'keel.doctor.run', view: 'list' },
  {
    group: 'system',
    section: 'Settings',
    tab: 'General',
    title: 'Settings',
    func: 'settings.list',
    view: 'list',
  },
  {
    group: 'system',
    section: 'Settings',
    tab: 'Brand by host',
    title: 'White-label by host name',
    func: 'settings.brand.list',
    view: 'list',
  },
  {
    group: 'system',
    section: 'Settings',
    tab: 'Delivery channels',
    title: 'Delivery channels',
    func: 'notify.channel.list',
    view: 'list',
  },
  {
    group: 'system',
    section: 'Settings',
    tab: 'Webhooks',
    title: 'Webhooks',
    func: 'webhook.subscription.list',
    view: 'list',
  },
  {
    group: 'system',
    section: 'Settings',
    tab: 'Peers',
    title: 'Peers',
    func: 'peer.list',
    view: 'list',
  },
  { group: 'system', section: 'Backups', title: 'Backups', func: 'backup.list', view: 'list' },
  {
    group: 'system',
    section: 'Installation',
    tab: 'Products',
    title: 'Products',
    func: 'keel.product.list',
    view: 'list',
  },
  {
    group: 'system',
    section: 'Installation',
    tab: 'License',
    title: 'License',
    func: 'license.get',
    view: 'object',
  },
  {
    group: 'system',
    section: 'Installation',
    tab: 'Updates',
    title: 'Updates',
    func: 'update.check',
    view: 'object',
  },
];

export function systemScreen(func: string): SystemScreen {
  const screen = SYSTEM_SCREENS.find((item) => item.func === func);
  if (screen === undefined) {
    throw new Error(`${func} is not a platform screen of the menu`);
  }
  return screen;
}

export function sectionTabs(page: Page): Locator {
  return page.locator('phx-tab-group.section-tabs');
}

export function screenView(page: Page, screen: SystemScreen): Locator {
  return page.locator(`keel-${screen.view}[data-${screen.view}="${screen.func}"]`);
}

export function sidebarLink(page: Page, screen: SystemScreen): Locator {
  return page
    .locator(`phx-sidebar phx-sidebar-group.group[data-group="${screen.group}"]`)
    .getByRole('link', { name: screen.section, exact: true });
}

export async function openSystemScreen(page: Page, screen: SystemScreen): Promise<void> {
  await sidebarLink(page, screen).click();
  if (screen.tab !== undefined) {
    await sectionTabs(page).getByRole('tab', { name: screen.tab, exact: true }).click();
  }
  await expect(page).toHaveURL(new RegExp(`/f/${screen.func.replaceAll('.', '\\.')}$`));
}
