const path = require('node:path');
const { getJestConfig } = require('@storybook/test-runner');

const base = getJestConfig();

module.exports = {
  ...base,
  rootDir: __dirname,
  modulePathIgnorePatterns: ['<rootDir>/dist', '<rootDir>/storybook-static'],
  setupFiles: [...(base.setupFiles ?? []), path.join(__dirname, '.storybook/test-runner-setup.js')],
};
