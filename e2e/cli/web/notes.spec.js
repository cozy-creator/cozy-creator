// notes (cl-1098): Paul, 11-16: "toast notifications should be one-horizontal line only, and expandable into a larger message. I.e., you
// click on them, and then you can open the full menu in a modal if you need to read it (like a lengthy error)" and "there should also be a
// button in the bottom left that can open up a history of notifications as well. That way you can go backa nd read ones you missed".
// Three text files dropped on the canvas are refused, each said in a note at the bottom left: every note one line, the long one cut with an
// ellipsis. Clicking it opens it in full above the Notifications button, its whole words and its error to copy, with no veil over the page
// and nothing over the canvas's middle; a selection let go outside keeps it, it doesn't go while open, Esc closes it and gives the keys
// back, Enter opens it again and a click outside closes it. Gone by themselves, the notes are counted on the button and listed in its
// history, newest first, and opening it clears the count; an entry opens in full, and Esc goes back to the list. Each at 1440×900 and
// 1920×1080, on paper and dark, kept for a person to look at: the stack, the note opened, the history.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, openPoint } = require('./fixtures');
const { dropFiles } = require('./hands');

const LONG = 'storyboard-for-the-harbour-sequence-third-act-night-exteriors-with-the-lighthouse-keeper-final-v7.txt';
const FILES = [LONG, 'notes.txt', 'todo.txt'].map((name) => ({ name, type: 'text/plain', bytes: Buffer.from(`${name}\n`.repeat(8)) }));
const REFUSED = "This file isn't an image, video or sound file.";
const LOOKS = [
  { width: 1440, height: 900, theme: 'paper' },
  { width: 1440, height: 900, theme: 'dark' },
  { width: 1920, height: 1080, theme: 'paper' },
  { width: 1920, height: 1080, theme: 'dark' },
];

test('notes', async ({ page, guards }) => {
  test.setTimeout(300_000);
  // The text files' imports, refused on purpose.
  guards.allow(/status of 400\b.*\/v1\/projects\/[^/ ]+\/imports$/);
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  const shots = join(process.env.OUT, 'web', 'notes');
  mkdirSync(shots, { recursive: true });
  const stack = page.locator('.notes .note:not(.notes-log .note)');
  const long = stack.filter({ hasText: LONG });
  const opened = page.getByRole('dialog', { name: 'Notification', exact: true });
  const history = page.getByRole('dialog', { name: 'Notifications', exact: true });
  const bell = page.getByRole('button', { name: 'Notifications', exact: true });
  const count = page.locator('.notes-bell .notes-count');
  const focusIn = (selector) => page.evaluate((selector) => Boolean(document.activeElement?.closest(selector)), selector);

  for (const [at, { width, height, theme }] of LOOKS.entries()) {
    const tag = `${width}x${height}-${theme}`;
    const first = at === 0;
    await page.setViewportSize({ width, height });
    await page.evaluate((one) => localStorage.setItem('cozy.theme', one), theme);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await canvasIdle(page);
    const away = await openPoint(page);

    if (first) await test.step('nothing said yet: the history says so', async () => {
      await expect.soft(bell, 'the button is there with no note').toBeVisible();
      if ((await bell.count()) && !(await count.count()) && !(await stack.count())) {
        await bell.click();
        await expect(history.getByText('No notifications yet.', { exact: true })).toBeVisible();
        await page.keyboard.press('Escape');
        await expect(history).toHaveCount(0);
      }
    });

    let order = [];
    await test.step(`${tag}: three refused files, a note each, one line each`, async () => {
      await dropFiles(page, FILES, away);
      await expect(stack.filter({ hasText: REFUSED }), 'a note for each file').toHaveCount(3);
      // Held while the pointer is on one, as a person reads them.
      await stack.filter({ hasText: 'todo.txt' }).locator('.line').hover();
      order = await stack.evaluateAll((all) => all.map((one) => one.querySelector('.line').textContent));
      await page.screenshot({ path: join(shots, `stack-${tag}.png`) });
      if (!first) return;
      const lines = await stack.evaluateAll((all) => all.map((one) => {
        const line = one.querySelector('.line'), style = getComputedStyle(line);
        return { text: line.textContent, height: one.getBoundingClientRect().height, lineHeight: parseFloat(style.lineHeight), cut: line.scrollWidth > line.clientWidth + 1, ellipsis: style.textOverflow === 'ellipsis' && style.whiteSpace === 'nowrap' };
      }));
      for (const one of lines) expect(one.height, `"${one.text.slice(0, 40)}…" is one line: ${JSON.stringify(one)}`).toBeLessThan(one.lineHeight * 2);
      const cut = lines.find((one) => one.text.startsWith(LONG));
      expect(cut, 'the long one keeps its whole words, cut with an ellipsis').toMatchObject({ text: `${LONG}: ${REFUSED}`, cut: true, ellipsis: true });
      expect(await long.getByRole('button', { name: 'Copy', exact: true }).count(), 'its copy icon on the line').toBe(1);
      expect(await long.getByRole('button', { name: 'Show all', exact: true }).count(), 'and that it opens').toBe(1);
      const box = await page.locator('.notes').boundingBox();
      expect(box.x <= 16 && height - (box.y + box.height) <= 24, `at the bottom left: ${JSON.stringify(box)}`).toBe(true);
    });

    await test.step(`${tag}: clicked, the long note opens in full`, async () => {
      await long.locator('.line').click();
      await expect(opened).toBeVisible();
      await expect(opened.locator('.note-open-message')).toHaveText(`${LONG}: ${REFUSED}`);
      await expect(opened.getByText('code: import_failed')).toBeVisible();
      await page.screenshot({ path: join(shots, `open-${tag}.png`) });
      if (!first) return;
      expect(await focusIn('.note-open'), 'the keys move into it').toBe(true);
      const seen = await page.evaluate(() => ({
        veiled: [...document.querySelectorAll('body *')].filter((el) => /blur/.test(getComputedStyle(el).filter + getComputedStyle(el).backdropFilter)).map((el) => el.className),
        middle: Boolean(document.elementFromPoint(innerWidth / 2, innerHeight / 2)?.closest('.notes')),
        at: document.querySelector('.note-open').getBoundingClientRect().toJSON(),
      }));
      expect(seen.veiled, 'no blur over the page').toEqual([]);
      expect(seen.middle, `nothing over the canvas's middle: ${JSON.stringify(seen.at)}`).toBe(false);
      await opened.getByRole('button', { name: 'Copy', exact: true }).click();
      await expect(opened.getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
      const copied = await page.evaluate(() => navigator.clipboard.readText());
      expect(copied.split('\n'), 'the whole error on the clipboard').toEqual(expect.arrayContaining([`${LONG}: ${REFUSED}`, 'code: import_failed']));
      // Its words selected from inside and let go outside: still open (Paul, 11-12).
      const words = await opened.locator('.note-open-message').boundingBox();
      await page.mouse.move(words.x + 4, words.y + words.height / 2);
      await page.mouse.down();
      await page.mouse.move(away.x, away.y, { steps: 8 });
      await page.mouse.up();
      expect(await page.evaluate(() => String(window.getSelection())), 'words selected').not.toBe('');
      await expect(opened, 'let go outside, still open').toBeVisible();
      // Past its 6 s, open: it stays.
      await page.waitForTimeout(8_000);
      await expect(opened, 'open, it stays past its time').toBeVisible();
    });

    await test.step(`${tag}: Esc closes it, and the keys go back`, async () => {
      await page.keyboard.press('Escape');
      await expect(opened).toHaveCount(0);
      if (!first) return;
      await expect(long, 'it went nowhere while it was open').toHaveCount(1);
      expect(await focusIn(`.note-stack .note-more`), 'the keys back on it').toBe(true);
      // From the keys, it opens again; a click outside closes it.
      await page.keyboard.press('Enter');
      await expect(opened).toBeVisible();
      await page.mouse.click(away.x, away.y);
      await expect(opened, 'a click outside closes it').toHaveCount(0);
      await page.evaluate(() => window.getSelection().removeAllRanges());
    });

    await test.step(`${tag}: the history lists them, newest first, and opens one in full`, async () => {
      if (first) {
        await page.mouse.move(away.x, away.y);
        await expect(stack, 'let go, they go by themselves').toHaveCount(0, { timeout: 20_000 });
        await expect(count, 'counted on the button').toHaveText('3');
      }
      await bell.click();
      await expect(history).toBeVisible();
      await expect(count, 'opened, the count clears').toHaveCount(0);
      const listed = history.locator('li .line');
      await expect(listed).toHaveCount(3);
      expect(await listed.allTextContents(), 'newest first').toEqual([...order].reverse());
      await page.screenshot({ path: join(shots, `history-${tag}.png`) });
      if (!first) {
        await page.keyboard.press('Escape');
        await expect(history).toHaveCount(0);
        return;
      }
      expect(await focusIn('.notes-log'), 'the keys move into it').toBe(true);
      await history.locator('li .note', { hasText: LONG }).locator('.line').click();
      await expect(opened.locator('.note-open-message')).toHaveText(`${LONG}: ${REFUSED}`);
      await expect(opened.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(history, 'Esc goes back to the list').toBeVisible();
      await page.keyboard.press('Escape');
      await expect(history).toHaveCount(0);
      expect(await focusIn('.notes-bell'), 'the keys back on the button').toBe(true);
    });
  }
});
