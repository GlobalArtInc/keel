import { isIconName } from '@globalart/keel/forms';
import {
  lookup,
  type Boot,
  type BootBrand,
  type BootMenuGroup,
  type BootMenuItem,
  type BootProduct,
  type Level,
  type Messages,
} from '@globalart/keel/meta';
import { SCREEN_PATH } from '@globalart/keel/renderer';
import type { PhxIconName } from '@globalart/phoenix-ui';

export const PLATFORM_LEVEL_PREFIX = 'keel.level';
export const PRODUCT_PREFIX = 'product';

export type Translate = (key: string) => string;

export interface MenuEntry {
  func: string;
  url: string;
  label: string;
  icon: PhxIconName | null;
}

export interface SidebarGroup {
  id: string;
  label: string;
  items: MenuEntry[];
}

export interface SidebarSection {
  product: string | null;
  label: string | null;
  groups: SidebarGroup[];
}

export interface BrandView {
  name: string;
  logo: string | null;
  mark: string;
}

export function menuEntries(items: readonly BootMenuItem[], label: Translate): MenuEntry[] {
  return items.map((item): MenuEntry => ({
    func: item.func,
    url: `/${SCREEN_PATH}/${item.func}`,
    label: label(item.label),
    icon: isIconName(item.icon) ? item.icon : null,
  }));
}

export function productGroups(menu: readonly BootMenuGroup[], product: string): BootMenuGroup[] {
  return menu.filter((group) => group.product === product);
}

export function platformGroups(menu: readonly BootMenuGroup[]): BootMenuGroup[] {
  return menu.filter((group) => group.product === null || group.product === undefined);
}

function menuGroups(groups: readonly BootMenuGroup[], label: Translate): SidebarGroup[] {
  return groups
    .filter((group) => group.items.length > 0)
    .map((group) => ({
      id: group.id,
      label: label(group.label),
      items: menuEntries(group.items, label),
    }));
}

function productSection(
  product: BootProduct,
  menu: readonly BootMenuGroup[],
  headed: boolean,
  label: Translate,
): SidebarSection {
  return {
    product: product.id,
    label: headed ? label(product.name) : null,
    groups: menuGroups(productGroups(menu, product.id), label),
  };
}

export function sidebarOf(boot: Boot, label: Translate): SidebarSection[] {
  const headed = boot.products.length > 1;
  const products = boot.products.map((product) =>
    productSection(product, boot.menu, headed, label),
  );
  const platform: SidebarSection = {
    product: null,
    label: null,
    groups: menuGroups(platformGroups(boot.menu), label),
  };
  return [...products, platform].filter((section) => section.groups.length > 0);
}

export function brandOf(brand: BootBrand, label: Translate): BrandView {
  const name = label(brand.name);
  return {
    name,
    logo: brand.logo ?? null,
    mark: [...name][0]?.toLocaleUpperCase() ?? '',
  };
}

export function levelKey(products: readonly BootProduct[], level: Level): string {
  return products.length === 1
    ? `${PRODUCT_PREFIX}.${products[0].id}.level.${level}`
    : `${PLATFORM_LEVEL_PREFIX}.${level}`;
}

export function levelLabel(
  products: readonly BootProduct[],
  level: Level,
  messages: Messages,
  label: Translate,
): string {
  return lookup(messages, levelKey(products, level)) ?? label(`${PLATFORM_LEVEL_PREFIX}.${level}`);
}
