import { provideAnimations } from '@angular/platform-browser/animations';
import { registerKeelDumps } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';
import { withThemeByDataAttribute } from '@storybook/addon-themes';
import { applicationConfig, type Preview } from '@storybook/angular';
import { bundledDumps } from './dumps';

registerKeelDumps(bundledDumps());

const preview: Preview = {
  argTypesEnhancers: [
    (context) => {
      const argTypes = { ...context.argTypes };
      for (const [key, value] of Object.entries(context.initialArgs ?? {})) {
        if (typeof value !== 'function') {
          continue;
        }
        const argType = argTypes[key] ?? {};
        if (argType['control'] === false) {
          argTypes[key] = { ...argType, control: { disable: true } };
        } else if (!argType['action'] && !argType['control']) {
          argTypes[key] = { ...argType, action: key };
        }
      }
      return argTypes;
    },
  ],
  parameters: {
    layout: 'padded',
    backgrounds: { disable: true },
    a11y: { element: '#storybook-root' },
    controls: { expanded: true },
    options: {
      storySort: { order: ['Shell', 'Renderer'] },
    },
  },
  decorators: [
    applicationConfig({ providers: [provideAnimations(), providePhoenixIcons()] }),
    withThemeByDataAttribute({
      themes: { light: 'light', dark: 'dark' },
      defaultTheme: 'light',
      attributeName: 'data-theme',
    }),
  ],
};

export default preview;
