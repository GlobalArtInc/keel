import type { Config } from 'jest';

const config: Config = {
  maxWorkers: process.env['CI'] ? 2 : '50%',
  projects: [
    '<rootDir>/libs/keel/jest.config.ts',
    '<rootDir>/products/demo/jest.config.ts',
    '<rootDir>/products/dns/jest.config.ts',
    '<rootDir>/products/panel/jest.config.ts',
    '<rootDir>/apps/shell/jest.config.ts',
  ],
};

export default config;
