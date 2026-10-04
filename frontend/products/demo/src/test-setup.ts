import { setupZoneTestEnv } from 'jest-preset-angular/setup-env/zone';

setupZoneTestEnv({
  errorOnUnknownElements: true,
  errorOnUnknownProperties: true,
});

if (typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => undefined;
}

if (typeof Range.prototype.getClientRects !== 'function') {
  Range.prototype.getClientRects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList;
}

if (typeof Range.prototype.getBoundingClientRect !== 'function') {
  Range.prototype.getBoundingClientRect = () =>
    ({ x: 0, y: 0, width: 0, height: 0, top: 0, right: 0, bottom: 0, left: 0, toJSON: () => ({}) }) as DOMRect;
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
