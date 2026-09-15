import type { StorybookConfig } from '@storybook/angular';

const config: StorybookConfig = {
  stories: ['../libs/keel/**/*.stories.ts'],
  addons: ['@storybook/addon-a11y', '@storybook/addon-themes'],
  staticDirs: [
    { from: '../../products/demo/brand', to: '/brand' },
    { from: '../../products/demo/brand', to: '/brand/demo' },
  ],
  framework: {
    name: '@storybook/angular',
    options: {},
  },
};

export default config;
