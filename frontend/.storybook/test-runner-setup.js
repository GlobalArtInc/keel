const nodeModule = require('node:module');

try {
  Object.defineProperty(nodeModule, 'register', {
    value: () => undefined,
    configurable: true,
    writable: true,
  });
} catch {
  nodeModule.register = () => undefined;
}
