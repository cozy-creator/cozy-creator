import { ApiError, type Client } from "../../api/client"
import type { Quote, Rent, Rented } from "../../types"
import { signal } from "../../ui/signal"

/**
 * The one paid order this tab is asking, outside any component: closing the drawer doesn't forget it, and clicking twice can't place two.
 * One stated price is one key: `placing` is entered before anything awaits, the only way to place is from `quoted`, and asking again after a
 * lost answer is the same order, which Calcifer answers once.
 */
export type Order =
  | { state: "idle" }
  | { state: "quoting"; order: Rent }
  | { state: "quoted"; order: Rent; quote: Quote; key: string }
  | { state: "placing"; order: Rent; quote: Quote; key: string }
  /** The answer was lost, so it may stand: it asks again under its key, or the list shows it. */
  | { state: "uncertain"; order: Rent; quote: Quote; key: string; error: unknown }
  /** Definitely nothing was rented. */
  | { state: "refused"; order: Rent; error: unknown; rose: boolean }

export const order = signal<Order>({ state: "idle" })

/** Whether the answer says nothing about the order: it may have been placed. A refusal said by Calcifer rented nothing. */
export const lost = (error: unknown) => error instanceof TypeError || (error instanceof ApiError && (error.body.kind === "unavailable" || error.body.kind === "internal"))

export async function quote(client: Client, asked: Rent) {
  const now = order.get()
  if (now.state === "placing" || now.state === "uncertain") return
  order.set({ state: "quoting", order: asked })
  try {
    const quote = await client.rentalQuote(asked)
    // Asked again meanwhile, for something else: that one's answer counts.
    if (order.get().state === "quoting" && (order.get() as { order: Rent }).order === asked) {
      order.set({ state: "quoted", order: asked, quote, key: `rent-${crypto.randomUUID()}` })
    }
  } catch (error) {
    order.set({ state: "refused", order: asked, error, rose: false })
  }
}

/** The stated price's one click, or asking again under the same key after a lost answer. Nothing else calls `rent`. */
export async function place(client: Client): Promise<Rented | undefined> {
  const now = order.get()
  if (now.state !== "quoted" && now.state !== "uncertain") return undefined
  const { order: asked, quote: stated, key } = now
  // With nothing to hold it to, an order would rent at any price: Calcifer says what's agreed, or nothing is ordered.
  const agreed = stated.agreed
  if (!agreed) return undefined
  order.set({ state: "placing", order: asked, quote: stated, key })
  try {
    const rented = await client.rent(asked, agreed, key)
    order.set({ state: "idle" })
    return rented
  } catch (error) {
    if (lost(error)) order.set({ state: "uncertain", order: asked, quote: stated, key, error })
    else order.set({ state: "refused", order: asked, error, rose: error instanceof ApiError && error.body.code === "rental_price_rose" })
    return undefined
  }
}

export const dismiss = () => order.set({ state: "idle" })
