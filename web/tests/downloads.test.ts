import { describe, expect, test } from "bun:test"
import {
  DESKTOP_BUILDS,
  RELEASES_URL,
  assetName,
  downloadUrl,
  guessOs,
  installerAsset,
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

  test("finds the installer a person actually wants", () => {
    // Installer names are bundler-generated and carry the version, so they are
    // matched rather than constructed; the preferred extension wins.
    const mac = DESKTOP_BUILDS.find((b) => b.target === "aarch64-apple-darwin")!
    const found = installerAsset(mac, {
      tag: "v1.2.3",
      assets: {
        "Yunova_1.2.3_aarch64.dmg": "https://example.test/a.dmg",
        "Yunova_aarch64.app.tar.gz": "https://example.test/a.app.tar.gz",
      },
      publishedAt: null,
    })
    expect(found?.ext).toBe("dmg")
    expect(found?.url).toBe("https://example.test/a.dmg")
  })

  test("never offers an installer built for the other CPU family", () => {
    // The two macOS builds differ only in architecture, so a loose match would
    // hand an Intel .dmg to an Apple silicon Mac — it installs and then will
    // not run.
    const arm = DESKTOP_BUILDS.find((b) => b.target === "aarch64-apple-darwin")!
    const release = {
      tag: "v1.2.3",
      assets: { "Yunova_1.2.3_x64.dmg": "https://example.test/intel.dmg" },
      publishedAt: null,
    }
    expect(installerAsset(arm, release)).toBeNull()

    const intel = DESKTOP_BUILDS.find((b) => b.target === "x86_64-apple-darwin")!
    expect(installerAsset(intel, release)?.url).toBe("https://example.test/intel.dmg")
  })

  test("ignores updater signatures, which are not downloads", () => {
    const linux = DESKTOP_BUILDS.find((b) => b.target === "x86_64-unknown-linux-gnu")!
    const found = installerAsset(linux, {
      tag: "v1.2.3",
      assets: {
        "yunova_1.2.3_amd64.AppImage.sig": "https://example.test/sig",
        "yunova_1.2.3_amd64.AppImage": "https://example.test/app",
      },
      publishedAt: null,
    })
    expect(found?.url).toBe("https://example.test/app")
  })

  test("reports no installer rather than a dead link", () => {
    // A release that published only the bare binaries must degrade to the
    // binary download, not render a button that 404s.
    const build = DESKTOP_BUILDS[0]
    expect(installerAsset(build, null)).toBeNull()
    expect(
      installerAsset(build, { tag: "v1.0.0", assets: {}, publishedAt: null })
    ).toBeNull()
  })

  test("every build names its standalone archive and its installers", () => {
    // The headless binary is what a server runs; a build that names no
    // installer would leave that platform with no way to install the app.
    for (const b of DESKTOP_BUILDS) {
      expect(assetName(b)).toContain(b.target)
      expect(b.installers.length).toBeGreaterThan(0)
    }
  })
})
