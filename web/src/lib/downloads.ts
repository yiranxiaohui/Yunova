/**
 * Desktop app downloads.
 *
 * Binaries are published as GitHub Release assets rather than served by this
 * server: the server image has no reason to carry five platform builds, and a
 * self-hosted instance should not have to mirror them to let a user install
 * the client.
 *
 * The listing is therefore fetched from the public GitHub API at view time.
 * That call can fail — no egress, rate limit, an air-gapped deployment — so
 * every platform keeps a static fallback link to the "latest release" page,
 * which resolves server-side at GitHub and never goes stale.
 *
 * Each platform offers two shapes, because they answer different questions.
 * An *installer* is what a person wants: it registers the app, its icon and
 * its launcher, and the app then shows this site in its own window. A bare
 * *binary* is what a server wants, where `--headless` is the whole point and a
 * desktop entry would be meaningless. Naming both here keeps the download page
 * from having to know which is which.
 */

export const DESKTOP_REPO = "yiranxiaohui/Yunova"

export const RELEASES_URL = `https://github.com/${DESKTOP_REPO}/releases/latest`

export type DesktopOs = "macos" | "windows" | "linux"

/** What an asset is for: a person installing an app, or a server running it. */
export type DesktopKind = "installer" | "binary"

export interface DesktopBuild {
  /** Stable key, also the suffix of the published asset name. */
  target: string
  os: DesktopOs
  /** Human label for the CPU family, e.g. "Apple 芯片". */
  arch: string
  /** Archive extension of the standalone binary for this target. */
  ext: "tar.gz" | "zip"
  /**
   * Installer file extensions produced by the release workflow, most
   * preferred first. Several exist per platform on purpose — a `.deb` is
   * wrong on Fedora and an `.AppImage` is wrong on a managed desktop — so the
   * page can offer the usual one and still name the alternative.
   */
  installers: string[]
}

export const DESKTOP_BUILDS: DesktopBuild[] = [
  {
    target: "aarch64-apple-darwin",
    os: "macos",
    arch: "Apple 芯片",
    ext: "tar.gz",
    installers: ["dmg", "app.tar.gz"],
  },
  {
    target: "x86_64-apple-darwin",
    os: "macos",
    arch: "Intel",
    ext: "tar.gz",
    installers: ["dmg", "app.tar.gz"],
  },
  {
    target: "x86_64-pc-windows-msvc",
    os: "windows",
    arch: "x64",
    ext: "zip",
    installers: ["exe", "msi"],
  },
  {
    target: "x86_64-unknown-linux-gnu",
    os: "linux",
    arch: "x64",
    ext: "tar.gz",
    installers: ["AppImage", "deb"],
  },
  {
    target: "aarch64-unknown-linux-gnu",
    os: "linux",
    arch: "ARM64",
    ext: "tar.gz",
    installers: ["AppImage", "deb"],
  },
]

export const OS_LABEL: Record<DesktopOs, string> = {
  macos: "macOS",
  windows: "Windows",
  linux: "Linux",
}

/** Asset name of the standalone binary produced by the release workflow. */
export function assetName(b: DesktopBuild): string {
  return `yunova-desktop-${b.target}.${b.ext}`
}

export interface ReleaseInfo {
  tag: string
  /** Asset name → browser download URL, for the assets that exist. */
  assets: Record<string, string>
  publishedAt: string | null
}

interface GhRelease {
  tag_name?: string
  published_at?: string | null
  assets?: Array<{ name?: string; browser_download_url?: string }>
}

/** Fetch the latest published release, or `null` when GitHub is unreachable. */
export async function fetchLatestRelease(
  signal?: AbortSignal
): Promise<ReleaseInfo | null> {
  try {
    const res = await fetch(
      `https://api.github.com/repos/${DESKTOP_REPO}/releases/latest`,
      { signal, headers: { Accept: "application/vnd.github+json" } }
    )
    if (!res.ok) return null
    const body = (await res.json()) as GhRelease
    if (!body.tag_name) return null
    const assets: Record<string, string> = {}
    for (const a of body.assets ?? []) {
      if (a.name && a.browser_download_url) assets[a.name] = a.browser_download_url
    }
    return {
      tag: body.tag_name,
      assets,
      publishedAt: body.published_at ?? null,
    }
  } catch {
    return null
  }
}

/** Direct asset URL of the standalone binary, or the releases page. */
export function downloadUrl(b: DesktopBuild, release: ReleaseInfo | null): string {
  return release?.assets[assetName(b)] ?? RELEASES_URL
}

/**
 * The installer asset for a build, when the release happens to publish one.
 *
 * Installer names are bundler-generated and carry the version and their own
 * arch spelling (`Yunova_1.2.3_amd64.deb`, `Yunova_aarch64.app.tar.gz`), so
 * they are matched by extension and architecture rather than constructed. A
 * guessed exact name would silently produce a dead link on the first release
 * where a bundler changed its format — the failure this module exists to
 * avoid.
 */
export function installerAsset(
  b: DesktopBuild,
  release: ReleaseInfo | null
): { name: string; url: string; ext: string } | null {
  if (!release) return null
  const names = Object.keys(release.assets)
  for (const ext of b.installers) {
    const match = names.find(
      (n) => n.endsWith(`.${ext}`) && matchesArch(n, b) && !isSignature(n)
    )
    if (match) return { name: match, url: release.assets[match], ext }
  }
  return null
}

/** Updater signatures sit beside their artefact and are not downloads. */
function isSignature(name: string): boolean {
  return name.endsWith(".sig")
}

/**
 * Whether an installer name belongs to this build's CPU family.
 *
 * Each bundler spells the architecture its own way, and one platform's builds
 * differ only by that spelling, so offering an Intel `.dmg` to an Apple
 * silicon Mac is exactly the mistake this prevents.
 */
function matchesArch(name: string, b: DesktopBuild): boolean {
  const n = name.toLowerCase()
  const arm = b.target.startsWith("aarch64")
  const armTokens = ["aarch64", "arm64"]
  const x64Tokens = ["x86_64", "x64", "amd64"]
  const hasArm = armTokens.some((t) => n.includes(t))
  const hasX64 = x64Tokens.some((t) => n.includes(t))
  // A name carrying neither token cannot be attributed, so it is accepted for
  // the platform's primary (x64) build only: dropping it would hide the one
  // asset a single-arch release published.
  if (!hasArm && !hasX64) return !arm
  return arm ? hasArm : hasX64
}

/** Best guess at the visitor's platform, used to promote one button. */
export function guessOs(ua = navigator.userAgent): DesktopOs | null {
  const s = ua.toLowerCase()
  // Phones and tablets are remote controls, not execution targets. They are
  // checked first because both lie about the desktop OS: iOS says "like Mac
  // OS X" and Android says "linux".
  if (s.includes("iphone") || s.includes("ipad") || s.includes("android")) return null
  if (s.includes("windows")) return "windows"
  if (s.includes("mac os") || s.includes("macintosh")) return "macos"
  if (s.includes("linux")) return "linux"
  return null
}

/**
 * The build to offer first.
 *
 * A browser cannot read the CPU family, and `navigator.platform` reports
 * "MacIntel" even on Apple silicon under Rosetta-era shims, so the arch is a
 * guess: prefer Apple silicon on modern macOS and let the user pick the other
 * entry from the full list when the guess is wrong.
 */
export function preferredBuild(os: DesktopOs | null): DesktopBuild | null {
  if (!os) return null
  if (os === "macos") {
    return DESKTOP_BUILDS.find((b) => b.target === "aarch64-apple-darwin") ?? null
  }
  if (os === "windows") {
    return DESKTOP_BUILDS.find((b) => b.target === "x86_64-pc-windows-msvc") ?? null
  }
  return DESKTOP_BUILDS.find((b) => b.target === "x86_64-unknown-linux-gnu") ?? null
}
