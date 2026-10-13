import { createTask, expect, open, test } from './fixtures';

test('a continued turn shows the note the backend put in its input', async ({ page, backend }) => {
  await open(page, backend);
  await createTask(page, '继续失败的任务', 'FAKE:fail');
  await page.getByRole('button', { name: '继续', exact: true }).click();
  await expect(page.locator('.turn-divider')).toHaveCount(2);

  // Read the real turn, without calling the frontend parser or copying the runtime's wording.
  // No message is queued for this continuation, so its input is one header and the note.
  const input = await page.evaluate(async () => {
    const module = '/src/store.ts';
    const { useStore } = await import(module);
    const turns = Object.values(useStore.getState().thread.turns) as { id: string; input: string }[];
    return turns.sort((a, b) => a.id.localeCompare(b.id)).at(-1)!.input;
  });
  const note = input.slice(input.indexOf('\n') + 1);
  expect(note.trim()).not.toBe('');
  await expect(page.locator('.message.runtime').filter({ hasText: note })).toBeVisible();
});
