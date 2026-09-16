import { describe, expect, test } from "bun:test"
import {
  DESKTOP_BUILDS,
  RELEASES_URL,
  assetName,
  downloadUrl,
  guessOs,
  preferredBuild,
} from "../src/lib/downloads"

describe("desktop client downloads", () => {
  test("asset names match what the release workflow publishes", () => {
    // The workflow archives as `yunova-desktop-$target`; drifting from that
    // silently degrades every button into a link to the releases page.
    expect(assetName(DESKTOP_BUILDS[0])).toBe(
      `yunova-desktop-${DESKTOP_BUILDS[0].target}.tar.gz`
    )
    const win = DESKTOP_BUILDS.find((b) => b.os === "windows")!
    expect(assetName(win)).toBe("yunova-desktop-x86_64-pc-windows-msvc.zip")
  })

  test("falls back to the releases page when the asset is missing", () => {
    const build = DESKTOP_BUILDS[0]
    expect(downloadUrl(build, null)).toBe(RELEASES_URL)
    expect(downloadUrl(build, { tag: "v1.0.0", assets: {}, publishedAt: null })).toBe(
      RELEASES_URL
    )
  })

  test("uses the published asset URL when the release lists it", () => {
    const build = DESKTOP_BUILDS[0]
    const url = "https://example.test/download.tar.gz"
    expect(
      downloadUrl(build, {
        tag: "v1.0.0",
        assets: { [assetName(build)]: url },
        publishedAt: null,
      })
    ).toBe(url)
  })

  test("detects the visitor's OS from the user agent", () => {
    expect(guessOs("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("macos")
    expect(guessOs("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows")
    expect(guessOs("Mozilla/5.0 (X11; Linux x86_64)")).toBe("linux")
    // A phone is not an execution target, so no build is promoted there.
    expect(guessOs("Mozilla/5.0 (Linux; Android 14; Pixel 8)")).toBeNull()
    expect(guessOs("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)")).toBeNull()
  })

  test("promotes exactly one build per desktop OS", () => {
    for (const os of ["macos", "windows", "linux"] as const) {
      const b = preferredBuild(os)
      expect(b?.os).toBe(os)
    }
    expect(preferredBuild(null)).toBeNull()
  })
})
