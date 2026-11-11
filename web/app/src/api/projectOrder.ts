let order = 0

/** Order project read starts and write acknowledgements within this page, even on the same browser clock tick. */
export function nextProjectOrder(): number {
  return ++order
}
