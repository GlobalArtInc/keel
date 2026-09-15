import { getStoryContext, waitForPageReady, type TestRunnerConfig } from '@storybook/test-runner';
import { checkA11y, configureAxe, injectAxe } from 'axe-playwright';

const config: TestRunnerConfig = {
  async preVisit(page) {
    await injectAxe(page);
  },

  async postVisit(page, context) {
    const storyContext = await getStoryContext(page, context);
    const a11y = storyContext.parameters?.['a11y'];
    if (a11y?.disable) {
      return;
    }
    await waitForPageReady(page);
    await configureAxe(page, { rules: a11y?.config?.rules });
    await checkA11y(
      page,
      a11y?.element ?? '#storybook-root',
      {
        detailedReport: true,
        detailedReportOptions: { html: false },
        axeOptions: a11y?.options,
      },
      false,
      'v2',
    );
  },
};

export default config;
