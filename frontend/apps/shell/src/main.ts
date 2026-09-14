import { bootstrapKeel } from '@globalart/keel/shell';
import { SHELL_PRODUCTS } from './products';

bootstrapKeel({ products: SHELL_PRODUCTS }).catch((error: unknown) => console.error(error));
