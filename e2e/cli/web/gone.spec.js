// gone (cl-980, Paul 11-13: a tab went back and forth between "This project can't be opened yet" and / once its folder was deleted): a
// project's folder deleted under its open tab is said where the tab stands, once, in plain words ("This project's folder is gone: <path>");
// the tab stays there and the index lets the project go; a reload says the same, and stays. Its own cell, off the native lane: Windows can't
// delete a folder Calcifer holds open.
const { rmSync } = require('fs');
const { test, expect, recordAddress, stays } = require('./fixtures');

test.use({ projectOptions: { starter: true } });

test('gone', async ({ context, project, api, signIn, guards }) => {
  // Its reads answer that its folder is gone, as they should: Chrome says each in its console.
  guards.allow(new RegExp(`status of 404\\b.*/v1/projects/${project.id}(?:[/?][^ ]*)?$`));
  await context.addInitScript(recordAddress);
  const page = await context.newPage();
  await signIn(page, project.url());
  await expect(page.locator('.react-flow__node').first()).toBeVisible();
  await expect.soft(page.locator('.unread, .gate'), 'it opens').toHaveCount(0);
  const said = `This project's folder is gone: ${project.path}`;
  const here = { path: `/p/${project.code}`, moves: [] };

  await test.step('its folder deleted under the open tab: said in place, once, and the tab stays', async () => {
    rmSync(project.path, { recursive: true, force: true });
    await expect(page.locator('.unread h2', { hasText: said })).toBeVisible();
    expect.soft(await stays(page), 'it stays where it is').toEqual(here);
    await expect.soft(page.locator('.unread'), 'said once').toHaveCount(1);
    await expect.soft(page.locator('.gate'), 'and no gate').toHaveCount(0);
    expect.soft((await api.get('/v1/projects?limit=1000')).projects.map((row) => row.id), 'it leaves the index').not.toContain(project.id);
  });

  await test.step('a reload says the same, and stays', async () => {
    await page.reload();
    await expect(page.locator('.gate h2', { hasText: said })).toBeVisible();
    expect.soft(await stays(page), 'it stays where it is').toEqual(here);
  });
});
