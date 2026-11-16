// gone (cl-980, Paul 11-13: a tab went back and forth between "This project can't be opened yet" and / once its folder was deleted): a
// project's folder deleted under its open tab, as a file manager deletes one (to the trash, then emptied), is said where the tab stands,
// once, in plain words ("This project's folder is gone: <path>"), with no read of it refused: Calcifer says it, and the page asks nothing
// more. The tab stays there and the index lets the project go, keeping only where it was (cl-1031): a reload by its code finds no project
// here and says how to bring it back, and stays. Its own cell, off the native lane: Windows can't move a folder Calcifer holds open.
const { renameSync, rmSync } = require('fs');
const { test, expect, canvasIdle, recordAddress, stays, canvasNames, canvasVisible } = require('./fixtures');
const { camera, sameCamera } = require('./geometry');

test.use({ projectOptions: { starter: true } });

test('gone', async ({ context, project, api, signIn, guards }) => {
  await context.addInitScript(recordAddress);
  const page = await context.newPage();
  await signIn(page, project.url());
  await expect.poll(async () => { const names = await canvasNames(page); return names.length > 0 && await canvasVisible(page, names[0]); }).toBe(true);
  await expect.soft(page.locator('.unread, .gate'), 'it opens').toHaveCount(0);
  // Opened, the page fits it and keeps its view: that write lands before the folder goes, as a person deletes a folder after the page
  // has settled.
  await canvasIdle(page);
  await expect.poll(async () => {
    const [{ viewport }, now] = await Promise.all([api.get(project.at('/view')), camera(page)]);
    return Boolean(viewport) && sameCamera({ x: viewport.x, y: viewport.y, zoom: viewport.zoom }, now);
  }, { message: "the page's view saved" }).toBe(true);
  const said = `This project's folder is gone: ${project.path}`;
  const here = { path: `/p/${project.code}`, moves: [] };

  await test.step('its folder deleted under the open tab: said in place, once, and the tab stays', async () => {
    const trash = `${project.path}-trash`;
    renameSync(project.path, trash);
    rmSync(trash, { recursive: true, force: true });
    await expect(page.locator('.unread h2', { hasText: said })).toBeVisible();
    expect.soft(await stays(page), 'it stays where it is').toEqual(here);
    await expect.soft(page.locator('.unread'), 'said once').toHaveCount(1);
    await expect.soft(page.locator('.gate'), 'and no gate').toHaveCount(0);
    expect.soft((await api.get('/v1/projects?limit=1000')).projects.map((row) => row.id), 'it leaves the index').not.toContain(project.id);
  });

  await test.step('a reload finds no project by its code and says how to bring it back, and stays', async () => {
    // Its address is asked and not found: the browser logs that 404 itself.
    guards.allow(/status of 404\b.*\/v1\/projects\/[^/ ]+\/address\b/);
    await page.reload();
    await expect(page.locator('.gate h2', { hasText: "Couldn't open this project" })).toBeVisible();
    await expect.soft(page.locator('.gate', { hasText: 'open the folder again where it is now' }), 'how to bring it back').toBeVisible();
    expect.soft(await stays(page), 'it stays where it is').toEqual(here);
  });
});
