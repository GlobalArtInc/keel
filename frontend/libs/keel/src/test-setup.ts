import { setupZoneTestEnv } from 'jest-preset-angular/setup-env/zone';

setupZoneTestEnv({
  errorOnUnknownElements: true,
  errorOnUnknownProperties: true,
});

if (typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => undefined;
}

if (typeof globalThis.ResizeObserver !== 'function') {
  class IdleResizeObserver implements ResizeObserver {
    observe(): void {
      return;
    }

    unobserve(): void {
      return;
    }

    disconnect(): void {
      return;
    }
  }
  globalThis.ResizeObserver = IdleResizeObserver;
}

const CSS_PARSING = 'Could not parse CSS stylesheet';
const reportError = console.error.bind(console);
console.error = (...args: unknown[]) => {
  const [first] = args;
  if (typeof first === 'object' && first !== null && 'message' in first) {
    const { message } = first;
    if (typeof message === 'string' && message.includes(CSS_PARSING)) {
      return;
    }
  }
  reportError(...args);
};
