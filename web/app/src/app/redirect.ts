import type { createBrowserRouter } from "react-router-dom"

type Router = ReturnType<typeof createBrowserRouter>

/** One redirect (a `replace`) per thing a person does, and one as the page opens (cl-980): a page that sends itself on and back can't loop.
 *  A push is a person's own step and always goes. */
let spent = false

for (const type of ["pointerdown", "keydown", "drop", "popstate"]) {
  addEventListener(type, (event) => {
    if (event.isTrusted) spent = false
  }, true)
}

export function guardRedirects(router: Router): Router {
  const go = router.navigate.bind(router) as (to: unknown, options?: { replace?: boolean }) => Promise<void>
  router.navigate = ((to: unknown, options?: { replace?: boolean }) => {
    if (typeof to !== "number" && options?.replace) {
      if (spent) return Promise.resolve()
      spent = true
    }
    return go(to, options)
  }) as Router["navigate"]
  return router
}
