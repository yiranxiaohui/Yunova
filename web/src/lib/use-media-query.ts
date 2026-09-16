import { useEffect, useState } from "react"

/**
 * Subscribe to a CSS media query from React.
 *
 * Layout decisions that JavaScript owns — "is the sidebar allowed to collapse
 * into a rail right now?" — must agree with the CSS breakpoints, otherwise the
 * collapsed desktop state leaks into the mobile drawer, where a 4rem rail is
 * unusable. Reading the query here keeps that single source of truth in one
 * place instead of duplicating breakpoints as `hidden max-md:block` pairs on
 * every label.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => {
    if (typeof window === "undefined" || !window.matchMedia) return false
    return window.matchMedia(query).matches
  })

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return
    const mql = window.matchMedia(query)
    const onChange = () => setMatches(mql.matches)
    onChange()
    mql.addEventListener("change", onChange)
    return () => mql.removeEventListener("change", onChange)
  }, [query])

  return matches
}

/** Tailwind's `md` breakpoint, where the sidebar stops being a drawer. */
export function useIsDesktop(): boolean {
  return useMediaQuery("(min-width: 768px)")
}
