import { expect, keelTest } from '@globalart/keel/testing/playwright';
import { openSystemScreen, systemScreen } from './system-menu';

const STREAM = 'keel.logs.stream';

keelTest.describe('stream', () => {
  keelTest('root opens the kernel log from the menu and sees it live', async ({ page, keel }) => {
    await keel.asAdmin();
    await openSystemScreen(page, systemScreen(STREAM));

    const stream = page.locator(`keel-stream[data-stream="${STREAM}"]`);
    await expect(stream).toHaveAttribute('data-phase', 'live');
    await expect(stream.getByRole('heading', { level: 1 })).toHaveText('Log');
    await expect(stream.locator('phx-badge')).toHaveText('Live');
    const log = stream.getByRole('log', { name: 'Log' });
    await expect(log).toBeVisible();
    await expect(log.locator('.line').first()).toBeVisible();

    await stream.locator('phx-log-view button.toggle').click();
    await expect(stream.locator('phx-log-view button.toggle')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  });
});
