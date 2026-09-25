import type { KeelProduct } from '@globalart/keel/shell';

export const SHELL_PRODUCTS: readonly KeelProduct[] = [
  {
    id: 'demo',
    load: () => import('./chunks/demo').then((chunk) => chunk.provideKeelDemo()),
  },
  {
    id: 'dns',
    load: () => import('./chunks/dns').then((chunk) => chunk.provideKeelDns()),
  },
];
