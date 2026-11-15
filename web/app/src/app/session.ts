/** Old hash routes become paths before the history router is made. The page holds no session or token (cl-980): Calcifer answers
 *  its own page, by what the browser says of where a request came from, for the user it runs as. */
export function takeHashRoute() {
  if (location.hash.startsWith("#/")) history.replaceState(null, "", location.hash.slice(1))
}
