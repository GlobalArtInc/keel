import type { Config } from 'jest';

const config: Config = {
  projects: [
    '<rootDir>/libs/keel/jest.config.ts',
    '<rootDir>/products/demo/jest.config.ts',
    '<rootDir>/apps/shell/jest.config.ts',
  ],
};

export default config;
