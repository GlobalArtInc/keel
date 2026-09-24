import type { KeelProduct } from '@globalart/keel/shell';

export const SHELL_PRODUCTS: readonly KeelProduct[] = [
  {
    id: 'demo',
    load: () => import('@globalart/keel-demo').then((library) => library.provideKeelDemo()),
  },
  {
    id: 'dns',
    load: () => import('@globalart/keel-dns').then((library) => library.provideKeelDns()),
  },
];
