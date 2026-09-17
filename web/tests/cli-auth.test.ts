import { describe, expect, test } from "bun:test"
import { isCompleteCode, normalizeUserCode } from "../src/lib/cli-auth"

describe("CLI login codes", () => {
  test("accepts the code however the user happens to type it", () => {
    // The code is read off a terminal and typed into a browser, so case and
    // the dash are formatting, not secret. Rejecting these would turn a login
    // into a puzzle without adding any entropy.
    const canonical = "ACDE-FGHJ"
    expect(normalizeUserCode("ACDE-FGHJ")).toBe(canonical)
    expect(normalizeUserCode("acdefghj")).toBe(canonical)
    expect(normalizeUserCode(" acde-fghj ")).toBe(canonical)
    expect(normalizeUserCode("ACDE FGHJ")).toBe(canonical)
  })

  test("does not reshape a code of the wrong length", () => {
    // Padding a typo into some other valid-looking code would surface the
    // failure as "wrong account" instead of "you mistyped it".
    expect(normalizeUserCode("ABC")).toBe("ABC")
    expect(normalizeUserCode("ABCDEFGHIJ")).toBe("ABCDEFGHIJ")
  })

  test("only a complete code is submittable", () => {
    // The lookup endpoint is rate-limited and authenticated; sending it
    // half-typed codes would burn that budget and flash errors while the user
    // is still typing.
    expect(isCompleteCode("acde-fghj")).toBe(true)
    expect(isCompleteCode("ACDEFGHJ")).toBe(true)
    expect(isCompleteCode("ACDE-FGH")).toBe(false)
    expect(isCompleteCode("")).toBe(false)
  })

  test("matches the server's normalisation rule", () => {
    // Kept in step with `normalize_user_code` in src/cli_auth.rs: if the two
    // ever disagree, a code the browser accepts would not be found on the
    // server and the login would fail with "code does not exist".
    expect(normalizeUserCode("a1b2-c3d4")).toBe("A1B2-C3D4")
    // Punctuation is stripped rather than rejected, exactly as the server does.
    expect(normalizeUserCode("A1B2_C3D4")).toBe("A1B2-C3D4")
  })
})
